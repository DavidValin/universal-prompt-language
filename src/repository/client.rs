// repository_client.rs
//
// Repository client: configuration management (`~/.upl/repconfig`) and the
// network operations `login`, `push`, `pull`, `del`, plus `set-rep` / `get-rep`.
//
// All connections are TLS-secured when the configured repository was set with
// `--tls`; otherwise plain TCP is used. The wire protocol is the
// length-prefixed bincode protocol from `rep_protocol.rs`.

use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::process::Command;

use rustls::ClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

use crate::repository::protocol::{
    Request, Response, Visibility, cert_dir, prompts_dir, read_msg, repconfig_path,
    validate_name, write_msg,
};

// ---------------------------------------------------------------------------
// Client configuration (~/.upl/repconfig, TOML-ish via serde_json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct RepConfig {
    /// Repository endpoint, e.g. `host:port`.
    pub host: String,
    /// Whether to use TLS when talking to the repository.
    pub tls: bool,
    /// Optional GPG signing key id/fingerprint used at login.
    pub gpg_key: Option<String>,
    /// Last logged-in username.
    pub username: Option<String>,
    /// Last session token.
    pub token: Option<String>,
}

impl RepConfig {
    pub fn load() -> io::Result<Option<Self>> {
        let p = repconfig_path()?;
        if !p.exists() {
            return Ok(None);
        }
        let bytes = fs::read_to_string(&p)?;
        let cfg: RepConfig = serde_json::from_str(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(cfg))
    }

    pub fn require() -> Result<Self, String> {
        Self::load()
            .map_err(|e| format!("cannot read repconfig: {e}"))?
            .ok_or_else(|| "no repository configured; run `upl set-rep <host:port>`".to_string())
    }

    pub fn save(&self) -> io::Result<()> {
        let p = repconfig_path()?;
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&p, bytes)
    }
}

// ---------------------------------------------------------------------------
// set-rep / get-rep
// ---------------------------------------------------------------------------

/// `upl set-rep <host:port> [--tls] [gpg_key_file]`
pub fn set_rep(
    host: &str,
    tls: bool,
    gpg_key_file: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cfg = RepConfig::load().ok().flatten().unwrap_or_default();
    cfg.host = host.to_string();
    cfg.tls = tls;
    cfg.gpg_key = match gpg_key_file {
        Some(path) => {
            let content = fs::read_to_string(path)?;
            // Store the trimmed key id/fingerprint.
            Some(content.trim().to_string())
        }
        None => None,
    };
    cfg.save()?;
    // Persist the server's CA cert (if provided alongside via the cert dir) —
    // we rely on ~/.upl/rep/cert/ca.pem if present for verification.
    eprintln!("repository set to {host} (tls: {tls})");
    Ok(())
}

/// `upl get-rep`
pub fn get_rep() -> Result<(), Box<dyn std::error::Error>> {
    match RepConfig::load()? {
        Some(cfg) => {
            println!("host:    {}", cfg.host);
            println!("tls:     {}", cfg.tls);
            match &cfg.gpg_key {
                Some(k) => println!("gpg_key: {k}"),
                None => println!("gpg_key: (none)"),
            }
            match &cfg.username {
                Some(u) => println!("user:    {u}"),
                None => println!("user:    (none)"),
            }
            match &cfg.token {
                Some(_) => println!("token:   (set)"),
                None => println!("token:   (none)"),
            }
        }
        None => println!("no repository configured; run `upl set-rep <host:port>`"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/// A connected (optionally TLS-wrapped) stream to the repository.
struct Conn {
    inner: Box<dyn ReadWrite>,
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

impl Conn {
    fn send(&mut self, req: &Request) -> io::Result<()> {
        write_msg(&mut self.inner, req)
    }
    fn recv(&mut self) -> io::Result<Response> {
        read_msg(&mut self.inner)
    }
}

/// Accept any certificate — used only when no pinned CA is available.
#[derive(Debug)]
struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

fn connect(cfg: &RepConfig) -> io::Result<Conn> {
    let stream = TcpStream::connect(&cfg.host)?;
    if !cfg.tls {
        return Ok(Conn {
            inner: Box::new(stream),
        });
    }
    // Install crypto provider (idempotent).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let ca_path = cert_dir()?.join("ca.pem");
    let builder = ClientConfig::builder();
    let client_config = if ca_path.exists() {
        // Verify against the pinned CA.
        let pem = fs::read(&ca_path)?;
        let mut reader = io::BufReader::new(pem.as_slice());
        let roots: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
            .collect::<Result<_, _>>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut root_store = rustls::RootCertStore::empty();
        for cert in roots {
            let _ = root_store.add(cert);
        }
        builder
            .with_root_certificates(root_store)
            .with_no_client_auth()
    } else {
        eprintln!(
            "warning: no CA cert at {}; accepting server cert unverified",
            ca_path.display()
        );
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    };
    let server_name = parse_server_name(&cfg.host)?;
    let conn = rustls::ClientConnection::new(Arc::new(client_config), server_name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tls = rustls::StreamOwned::new(conn, stream);
    Ok(Conn {
        inner: Box::new(tls),
    })
}

use std::sync::Arc;

/// Derive a `ServerName` from a `host:port` string (strips the port).
fn parse_server_name(host_port: &str) -> io::Result<ServerName<'static>> {
    let host = host_port.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_port);
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        Ok(ServerName::IpAddress(ip.into()))
    } else {
        ServerName::try_from(host.to_string())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }
}

/// Run a single request/response round against the configured repository.
fn round_trip(cfg: &RepConfig, req: Request) -> Result<Response, String> {
    let mut conn = connect(cfg).map_err(|e| format!("connect to {} failed: {e}", cfg.host))?;
    conn.send(&req).map_err(|e| format!("send failed: {e}"))?;
    conn.recv().map_err(|e| format!("recv failed: {e}"))
}

// ---------------------------------------------------------------------------
// login
// ---------------------------------------------------------------------------

/// `upl login` — authenticates against the configured repository and stores
/// the resulting token in `~/.upl/repconfig`.
pub fn login(username: &str, password: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut cfg = RepConfig::require()?;

    // The login flow is a two-step challenge/response that must run on a
    // single connection (the server keeps the nonce in per-connection
    // state), so we don't use `round_trip` here.
    let mut conn = connect(&cfg).map_err(|e| format!("connect to {} failed: {e}", cfg.host))?;
    conn.send(&Request::LoginStart {
        username: username.to_string(),
    })?;
    let resp = conn.recv()?;
    let (nonce, requires_gpg) = match resp {
        Response::Challenge { nonce, requires_gpg } => (nonce, requires_gpg),
        Response::Error { message, .. } => {
            return Err(format!("login failed: {message}").into());
        }
        _ => return Err("unexpected server response".into()),
    };

    let gpg_signature = match (&cfg.gpg_key, requires_gpg) {
        (Some(key_id), true) => Some(sign_with_gpg(key_id, &nonce)?),
        (None, true) => {
            return Err(
                "server requires a GPG signature but none configured (use `set-rep ... <gpg_key_file>`)"
                    .into(),
            );
        }
        _ => None,
    };

    conn.send(&Request::LoginFinish {
        password: password.to_string(),
        gpg_signature,
    })?;
    let resp = conn.recv()?;
    match resp {
        Response::LoginOk { token } => {
            cfg.username = Some(username.to_string());
            cfg.token = Some(token);
            cfg.save()?;
            eprintln!("logged in as '{username}'");
            Ok(())
        }
        Response::Error { message, .. } => Err(format!("login failed: {message}").into()),
        _ => Err("unexpected server response".into()),
    }
}

/// Sign `data` with the user's GPG private key identified by `key_id` and
/// return the detached (binary) signature.
fn sign_with_gpg(key_id: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = Command::new("gpg")
        .args([
            "--batch",
            "--yes",
            "--detach-sign",
            "--local-user",
            key_id,
            "--output",
            "-", // write signature to stdout
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to run gpg: {e}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("gpg stdin unavailable")?;
        stdin.write_all(data).map_err(|e| format!("gpg stdin: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("gpg wait: {e}"))?;
    if !output.status.success() {
        return Err("gpg signing failed".to_string());
    }
    Ok(output.stdout)
}

// ---------------------------------------------------------------------------
// push
// ---------------------------------------------------------------------------

/// `upl push <local_prompt.upl|txt> [--visibility public|private]`
///
/// The prompt's parsed `name` is used as the repository name. The file MUST
/// use the `.txt` or `.upl` extension and its base name MUST equal the
/// prompt's `name` field (RFC §2).
pub fn push(
    file: &str,
    visibility: Visibility,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = RepConfig::require()?;
    let token = cfg
        .token
        .clone()
        .ok_or("not logged in; run `upl login`")?;

    let path = std::path::Path::new(file);
    let content = fs::read(file)?;
    // Parse to obtain the `name` (which is used as the repository name) and
    // to enforce the RFC §2 file-format rules (extension + name/file match).
    // The server re-validates, but failing early here gives a better error.
    let prompt = crate::upl::parser::PromptParser::parse(
        std::str::from_utf8(&content).map_err(|e| format!("prompt is not utf-8: {e}"))?,
    )
    .map_err(|e| format!("invalid prompt: {e}"))?;
    crate::upl::parser::validate_prompt_file(&prompt, path)
        .map_err(|e| format!("invalid prompt file: {e}"))?;
    let name = prompt.name;
    crate::repository::protocol::validate_name(&name)?;

    let resp = round_trip(&cfg, Request::Push {
        token,
        name: name.clone(),
        visibility,
        content,
    })?;
    match resp {
        Response::PushOk { version } => {
            eprintln!("pushed '{name}' as version {version}");
            Ok(())
        }
        Response::Error { message, .. } => Err(format!("push failed: {message}").into()),
        _ => Err("unexpected server response".into()),
    }
}

// ---------------------------------------------------------------------------
// pull
// ---------------------------------------------------------------------------

/// `upl pull <username>/<prompt_name>[/<version>]`
///
/// Stores the result in `~/.upl/prompts/<host>/<username>/<name>.txt`,
/// injecting a `source:` metadata field set to `<host>/<username>/<prompt_name>`.
pub fn pull(spec: &str) -> Result<(), Box<dyn std::error::Error>> {
    let parts: Vec<&str> = spec.split('/').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err("usage: upl pull <username>/<prompt_name>[/<version>]".into());
    }
    let username = parts[0];
    let name = parts[1];
    let version = if parts.len() == 3 {
        Some(parts[2].parse::<u32>().map_err(|_| "invalid version number")?)
    } else {
        None
    };
    validate_name(name)?;

    let cfg = RepConfig::require()?;

    let resp = round_trip(&cfg, Request::Pull {
        username: username.to_string(),
        name: name.to_string(),
        version,
        token: cfg.token.clone(),
    })?;
    let (version, content) = match resp {
        Response::PullOk { version, content } => (version, content),
        Response::Error { message, .. } => return Err(format!("pull failed: {message}").into()),
        _ => return Err("unexpected server response".into()),
    };

    // Inject the `source` metadata field.
    let source = format!("{}/{}/{}", cfg.host, username, name);
    let text = String::from_utf8(content).map_err(|e| format!("prompt is not utf-8: {e}"))?;
    let injected = inject_source(&text, &source);

    // Write to ~/.upl/prompts/<host>/<username>/<name>.txt
    // This namespaces by host+user so prompts from different repositories
    // or different authors never collide. The flat ~/.upl/prompts/
    // top level is reserved for locally-authored prompts.
    let dir = prompts_dir()?.join(&cfg.host).join(username);
    fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{name}.txt"));
    fs::write(&out, injected.as_bytes())?;
    eprintln!("pulled '{name}' version {version} -> {}", out.display());
    Ok(())
}

/// Insert a `source:` header field into a UPL document. The header begins
/// after an optional leading `--` line; the field is placed immediately after
/// that opening delimiter (or prepended with one if absent).
fn inject_source(content: &str, source: &str) -> String {
    let line = format!("source: {source}\n");
    if let Some(rest) = content.strip_prefix("--\n") {
        format!("--\n{line}{rest}")
    } else if let Some(rest) = content.strip_prefix("--\r\n") {
        format!("--\r\n{line}{rest}")
    } else {
        // No opening delimiter: prepend one with the source field.
        format!("--\n{line}{content}")
    }
}

// ---------------------------------------------------------------------------
// del
// ---------------------------------------------------------------------------

/// `upl del <prompt_name>` — deletes all versions of a prompt in the
/// authenticated user's own account.
pub fn del(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = RepConfig::require()?;
    let token = cfg
        .token
        .clone()
        .ok_or("not logged in; run `upl login`")?;
    let resp = round_trip(&cfg, Request::Delete {
        token,
        name: name.to_string(),
    })?;
    match resp {
        Response::Ok => {
            eprintln!("deleted '{name}'");
            Ok(())
        }
        Response::Error { message, .. } => Err(format!("delete failed: {message}").into()),
        _ => Err("unexpected server response".into()),
    }
}