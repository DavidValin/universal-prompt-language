// repository_server.rs
//
// TCP repository server (with TLS) for the upl prompts repository, plus the
// local admin operations `register_user` / `delete_user`.
//
// The server speaks the length-prefixed bincode protocol defined in
// `rep_protocol.rs`. It stores prompts under `~/.upl/rep/<user>/<name>/`
// and credentials (argon2id) under `~/.upl/rep/cred/`.
//
// The same binary acts as server (`upl start_repository <tls_cert>`),
// local admin (`upl register_user`, `upl delete_user <name>`), and
// repository client (`upl login/push/pull/del`).

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::repository::protocol::{
    self, Credential, PromptMeta, Request, Response, Visibility, cert_dir, err_code, prompt_dir,
    prompt_version_path, random_nonce, random_token, read_msg, repconfig_path, user_dir,
    validate_name, validate_prompt_content, write_msg,
};

/// Default bind address for `start_repository` when none is configured.
pub const DEFAULT_BIND: &str = "0.0.0.0:7654";

/// Server-side shared state across all connections.
struct ServerState {
    /// session token -> username
    tokens: Mutex<HashMap<String, String>>,
    /// per-(user,name) push lock: serializes version increments
    push_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ServerState {
    fn new() -> Self {
        ServerState {
            tokens: Mutex::new(HashMap::new()),
            push_locks: Mutex::new(HashMap::new()),
        }
    }

    fn issue_token(&self, username: &str) -> String {
        let token = random_token();
        self.tokens
            .lock()
            .unwrap()
            .insert(token.clone(), username.to_string());
        token
    }

    fn resolve_token(&self, token: &str) -> Option<String> {
        self.tokens.lock().unwrap().get(token).cloned()
    }

    fn lock_for(&self, user: &str, name: &str) -> Arc<Mutex<()>> {
        let key = format!("{user}/{name}");
        let mut map = self.push_locks.lock().unwrap();
        map.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    }
}

// ---------------------------------------------------------------------------
// TLS setup
// ---------------------------------------------------------------------------

/// Load all certificates from a PEM file.
fn load_certs(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<_, _>>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no certificates found in TLS cert file",
        ));
    }
    Ok(certs)
}

/// Load the first private key from a PEM file.
fn load_key(path: &Path) -> io::Result<PrivateKeyDer<'static>> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let keys = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    keys.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no private key found in TLS cert file"))
}

/// Read the configured bind address from `~/.upl/repconfig` (the `host`
/// field), falling back to `DEFAULT_BIND`.
fn configured_bind() -> String {
    if let Ok(p) = repconfig_path() {
        if let Ok(bytes) = fs::read_to_string(&p) {
            if let Ok(cfg) = serde_json::from_str::<RepConfigLite>(&bytes) {
                if let Some(h) = cfg.host {
                    return h;
                }
            }
        }
    }
    DEFAULT_BIND.to_string()
}

/// Minimal subset of the client config — we only need `host` to derive the
/// bind address.
#[derive(serde::Deserialize, Default)]
struct RepConfigLite {
    host: Option<String>,
}

// ---------------------------------------------------------------------------
// Entry point: start the repository server
// ---------------------------------------------------------------------------

/// `upl start_repository [tls_cert] [bind_addr]`
///
/// When a `tls_cert` PEM file is provided (cert chain + private key), the
/// server speaks TLS; otherwise it runs in plain TCP mode. The cert is
/// copied into `~/.upl/rep/cert/server.pem`.
pub fn start_repository(
    tls_cert: Option<&str>,
    bind: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Install the crypto provider (idempotent).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Ensure storage dirs exist.
    fs::create_dir_all(protocol::rep_dir()?)?;
    fs::create_dir_all(protocol::cred_dir()?)?;
    let cert_store = cert_dir()?;
    fs::create_dir_all(&cert_store)?;

    let tls_config = match tls_cert {
        Some(path) => {
            let cert_src = Path::new(path);
            if !cert_src.exists() {
                return Err(format!("TLS cert file not found: {path}").into());
            }
            let server_pem = cert_store.join("server.pem");
            fs::copy(cert_src, &server_pem)?;
            let certs = load_certs(&server_pem)?;
            let key = load_key(&server_pem)?;
            let config = ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Some(Arc::new(config))
        }
        None => None,
    };

    let bind_addr = bind.map(|s| s.to_string()).unwrap_or_else(configured_bind);
    let listener = TcpListener::bind(&bind_addr)?;
    let scheme = if tls_config.is_some() { "TLS" } else { "plain TCP" };
    eprintln!("upl repository server listening on {bind_addr} ({scheme})");
    serve(listener, tls_config)
}

/// Accept loop over a pre-bound listener. Public so tests can bind their own
/// listener (e.g. to port 0) and drive the server in-process.
pub fn serve(listener: TcpListener, tls_config: Option<Arc<ServerConfig>>) -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(ServerState::new());
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let state = state.clone();
        match tls_config {
            Some(ref acceptor) => {
                let acceptor = acceptor.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_conn_tls(stream, acceptor, state) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            None => {
                std::thread::spawn(move || {
                    if let Err(e) = handle_conn_plain(stream, state) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
        }
    }
    Ok(())
}

fn handle_conn_plain(stream: TcpStream, state: Arc<ServerState>) -> io::Result<()> {
    let mut stream = stream;
    handle_conn(&mut stream, &state)
}

fn handle_conn_tls(
    stream: TcpStream,
    acceptor: Arc<ServerConfig>,
    state: Arc<ServerState>,
) -> io::Result<()> {
    let conn = rustls::ServerConnection::new(acceptor)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut tls = rustls::StreamOwned::new(conn, stream);
    handle_conn(&mut tls, &state)
}

/// Handle a single connection's request/response loop.
fn handle_conn<S: Read + Write>(stream: &mut S, state: &ServerState) -> io::Result<()> {
    // Per-connection login challenge state.
    let mut pending_nonce: Option<Vec<u8>> = None;
    let mut pending_user: Option<String> = None;

    loop {
        let req: Request = match read_msg(stream) {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let resp = handle_request(req, state, &mut pending_nonce, &mut pending_user);
        write_msg(stream, &resp)?;
    }
}

fn handle_request(
    req: Request,
    state: &ServerState,
    pending_nonce: &mut Option<Vec<u8>>,
    pending_user: &mut Option<String>,
) -> Response {
    match req {
        Request::LoginStart { username } => {
            let requires_gpg = Credential::load(&username)
                .ok()
                .flatten()
                .map(|c| c.gpg_pubkey.is_some())
                .unwrap_or(false);
            let nonce = random_nonce(32);
            *pending_user = Some(username);
            *pending_nonce = Some(nonce.clone());
            Response::Challenge { nonce, requires_gpg }
        }
        Request::LoginFinish {
            password,
            gpg_signature,
        } => {
            let user = match pending_user.take() {
                Some(u) => u,
                None => return Response::err(err_code::BAD_REQUEST, "no pending login"),
            };
            let nonce = match pending_nonce.take() {
                Some(n) => n,
                None => return Response::err(err_code::BAD_REQUEST, "no pending challenge"),
            };
            let cred = match Credential::load(&user).ok().flatten() {
                Some(c) => c,
                None => {
                    return Response::err(err_code::UNAUTHORIZED, "invalid credentials");
                }
            };
            if !cred.verify_password(&password) {
                return Response::err(err_code::UNAUTHORIZED, "invalid credentials");
            }
            if let Some(pubkey) = &cred.gpg_pubkey {
                match gpg_signature {
                    Some(sig) => {
                        if !verify_gpg_signature(pubkey, &nonce, &sig) {
                            return Response::err(
                                err_code::UNAUTHORIZED,
                                "invalid GPG signature",
                            );
                        }
                    }
                    None => {
                        return Response::err(
                            err_code::UNAUTHORIZED,
                            "GPG signature required for this account",
                        );
                    }
                }
            }
            let token = state.issue_token(&user);
            Response::LoginOk { token }
        }
        Request::Push {
            token,
            name,
            visibility,
            content,
        } => {
            let user = match state.resolve_token(&token) {
                Some(u) => u,
                None => return Response::err(err_code::UNAUTHORIZED, "invalid or expired token"),
            };
            handle_push(state, &user, &name, visibility, &content)
        }
        Request::Pull {
            username,
            name,
            version,
            token,
        } => handle_pull(state, &username, &name, version, token),
        Request::Delete { token, name } => {
            let user = match state.resolve_token(&token) {
                Some(u) => u,
                None => return Response::err(err_code::UNAUTHORIZED, "invalid or expired token"),
            };
            handle_delete(&user, &name)
        }
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

fn handle_push(
    state: &ServerState,
    user: &str,
    name: &str,
    visibility: Visibility,
    content: &[u8],
) -> Response {
    if let Err(e) = validate_name(name) {
        return Response::err(err_code::BAD_REQUEST, e);
    }
    // Validate the prompt content and ensure its parsed `name` matches `name`.
    let parsed_name = match validate_prompt_content(content) {
        Ok(name) => name,
        Err(e) => return Response::err(err_code::INVALID_PROMPT, e),
    };
    if parsed_name != name {
        return Response::err(
            err_code::INVALID_PROMPT,
            format!("prompt name '{parsed_name}' does not match name '{name}'"),
        );
    }

    // Acquire the per-(user,name) lock to serialize version increments.
    let lock = state.lock_for(user, name);
    let _guard = lock.lock().unwrap();

    let dir = match prompt_dir(user, name) {
        Ok(d) => d,
        Err(e) => return Response::err(err_code::INTERNAL, e.to_string()),
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        return Response::err(err_code::INTERNAL, e.to_string());
    }

    // Compute the SHA-256 of the incoming content.
    let new_sha256 = sha256_hex(content);

    match PromptMeta::load(user, name) {
        Ok(Some(meta)) => {
            // If the content is identical to the latest version, don't
            // create a new version — just return the current one.
            if meta.latest_sha256 == new_sha256 {
                // Visibility may still change on a no-op push.
                let updated = PromptMeta {
                    visibility,
                    latest_version: meta.latest_version,
                    latest_sha256: meta.latest_sha256,
                };
                if let Err(e) = updated.save(user, name) {
                    return Response::err(err_code::INTERNAL, e.to_string());
                }
                return Response::PushOk {
                    version: meta.latest_version,
                };
            }
            // Content differs: create a new version.
            let next_version = meta.latest_version.saturating_add(1);
            let path = match prompt_version_path(user, name, next_version) {
                Ok(p) => p,
                Err(e) => return Response::err(err_code::INTERNAL, e.to_string()),
            };
            if let Err(e) = fs::write(&path, content) {
                return Response::err(err_code::INTERNAL, e.to_string());
            }
            let meta = PromptMeta {
                visibility,
                latest_version: next_version,
                latest_sha256: new_sha256,
            };
            if let Err(e) = meta.save(user, name) {
                return Response::err(err_code::INTERNAL, e.to_string());
            }
            Response::PushOk {
                version: next_version,
            }
        }
        Ok(None) => {
            // First push: version 1.
            let path = match prompt_version_path(user, name, 1) {
                Ok(p) => p,
                Err(e) => return Response::err(err_code::INTERNAL, e.to_string()),
            };
            if let Err(e) = fs::write(&path, content) {
                return Response::err(err_code::INTERNAL, e.to_string());
            }
            let meta = PromptMeta {
                visibility,
                latest_version: 1,
                latest_sha256: new_sha256,
            };
            if let Err(e) = meta.save(user, name) {
                return Response::err(err_code::INTERNAL, e.to_string());
            }
            Response::PushOk { version: 1 }
        }
        Err(e) => Response::err(err_code::INTERNAL, e.to_string()),
    }
}

/// Compute the SHA-256 of `data` as a lowercase hex string.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn handle_pull(
    _state: &ServerState,
    user: &str,
    name: &str,
    version: Option<u32>,
    token: Option<String>,
) -> Response {
    if let Err(e) = validate_name(name) {
        return Response::err(err_code::BAD_REQUEST, e);
    }
    let meta = match PromptMeta::load(user, name) {
        Ok(Some(m)) => m,
        Ok(None) => return Response::err(err_code::NOT_FOUND, "prompt not found"),
        Err(e) => return Response::err(err_code::INTERNAL, e.to_string()),
    };

    // Enforce visibility: private prompts require the owner's token.
    if meta.visibility == Visibility::Private {
        match token.as_ref().and_then(|t| _state.resolve_token(t)) {
            Some(u) if u == user => {}
            _ => {
                return Response::err(
                    err_code::FORBIDDEN,
                    "prompt is private; valid owner token required",
                );
            }
        }
    }

    let resolved = match version {
        Some(v) => v,
        None => meta.latest_version,
    };
    if resolved == 0 || resolved > meta.latest_version {
        return Response::err(
            err_code::NOT_FOUND,
            format!("version {resolved} not available (latest: {})", meta.latest_version),
        );
    }
    let path = match prompt_version_path(user, name, resolved) {
        Ok(p) => p,
        Err(e) => return Response::err(err_code::INTERNAL, e.to_string()),
    };
    match fs::read(&path) {
        Ok(content) => Response::PullOk {
            version: resolved,
            content,
        },
        Err(e) => Response::err(err_code::NOT_FOUND, e.to_string()),
    }
}

fn handle_delete(user: &str, name: &str) -> Response {
    if let Err(e) = validate_name(name) {
        return Response::err(err_code::BAD_REQUEST, e);
    }
    let dir = match prompt_dir(user, name) {
        Ok(d) => d,
        Err(e) => return Response::err(err_code::INTERNAL, e.to_string()),
    };
    if !dir.exists() {
        return Response::err(err_code::NOT_FOUND, "prompt not found");
    }
    match fs::remove_dir_all(&dir) {
        Ok(()) => Response::Ok,
        Err(e) => Response::err(err_code::INTERNAL, e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// GPG signature verification (shells out to the system `gpg`)
// ---------------------------------------------------------------------------

/// Verify that `signature` is a valid detached signature of `data` made with
/// a key matching the armored `pubkey`. Uses a temporary GNUPGHOME so the
/// server's real keyring is never touched.
fn verify_gpg_signature(pubkey_armored: &str, data: &[u8], signature: &[u8]) -> bool {
    let tmp = match tempfile_dir() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let pubkey_path = tmp.join("pub.asc");
    let data_path = tmp.join("data.bin");
    let sig_path = tmp.join("sig.bin");
    if fs::write(&pubkey_path, pubkey_armored).is_err()
        || fs::write(&data_path, data).is_err()
        || fs::write(&sig_path, signature).is_err()
    {
        return false;
    }
    // Import the public key into the throwaway keyring.
    let import = Command::new("gpg")
        .args([
            "--homedir",
            tmp.to_str().unwrap_or(""),
            "--batch",
            "--yes",
            "--import",
            pubkey_path.to_str().unwrap_or(""),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !matches!(import, Ok(s) if s.success()) {
        return false;
    }
    // Verify the detached signature.
    let verify = Command::new("gpg")
        .args([
            "--homedir",
            tmp.to_str().unwrap_or(""),
            "--batch",
            "--yes",
            "--verify",
            sig_path.to_str().unwrap_or(""),
            data_path.to_str().unwrap_or(""),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    matches!(verify, Ok(s) if s.success())
}

/// Create a unique temporary directory.
fn tempfile_dir() -> io::Result<PathBuf> {
    let base = std::env::temp_dir();
    let name = format!("upl-gpg-{}", random_nonce(8).iter().map(|b| format!("{b:02x}")).collect::<String>());
    let dir = base.join(name);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// Local admin operations
// ---------------------------------------------------------------------------

/// `upl register_user` (local admin). Creates or updates a credential.
pub fn register_user(
    username: &str,
    password: &str,
    gpg_key_file: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Err(e) = validate_name(username) {
        return Err(format!("invalid username: {e}").into());
    }
    fs::create_dir_all(protocol::cred_dir()?)?;

    let password_hash = Credential::hash_password(password)
        .map_err(io::Error::other)?;

    let gpg_pubkey = match gpg_key_file {
        Some(path) => {
            let key_path = Path::new(path);
            // If the file looks like an armored public key, use it directly.
            // Otherwise assume it is a key id/fingerprint and export the
            // public key from the user's keyring.
            let content = fs::read_to_string(key_path)?;
            let armored = if content.contains("BEGIN PGP PUBLIC KEY BLOCK") {
                content
            } else {
                export_gpg_pubkey(&content)?
            };
            Some(armored)
        }
        None => None,
    };

    let cred = Credential {
        password_hash,
        gpg_pubkey,
    };
    cred.save(username)?;
    eprintln!("registered user '{username}'");
    Ok(())
}

/// `upl delete_user <username>` (local admin). Removes a credential and its
/// prompt directory.
pub fn delete_user(username: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Err(e) = validate_name(username) {
        return Err(format!("invalid username: {e}").into());
    }
    Credential::delete(username)?;
    // Remove the user's prompt directory if present.
    let dir = user_dir(username)?;
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    eprintln!("deleted user '{username}'");
    Ok(())
}

/// Export an armored public key for the given key id/fingerprint using the
/// user's gpg keyring.
fn export_gpg_pubkey(key_id: &str) -> io::Result<String> {
    let output = Command::new("gpg")
        .args(["--armor", "--export", key_id])
        .output()?;
    if !output.status.success() {
            return Err(io::Error::other(format!(
                "gpg --export failed for '{key_id}'"
            )));
    }
    let s = String::from_utf8_lossy(&output.stdout).into_owned();
    if !s.contains("BEGIN PGP PUBLIC KEY BLOCK") {
        return Err(io::Error::other(format!(
            "no public key exported for '{key_id}'"
        )));
    }
    Ok(s)
}