// rep_protocol.rs
//
// Shared types and helpers for the prompts repository (server + client):
//   - paths under ~/.upl
//   - wire protocol (length-prefixed bincode)
//   - credential storage (argon2id)
//   - prompt visibility / limits
//
// Both `repository_server.rs` and `repository_client.rs` depend on this module.

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::upl::parser::PromptParser;

pub const TOKEN_LEN: usize = 32;
pub const MAX_NAME_LEN: usize = 255;
pub const MAX_PROMPT_BYTES: usize = 20 * 1024 * 1024; // 20 MB

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn home_dir() -> io::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME not set"))
}

/// `~/.upl`
pub fn upl_home() -> io::Result<PathBuf> {
    Ok(home_dir()?.join(".upl"))
}

/// `~/.upl/rep` — server data root (prompts + credentials + cert).
pub fn rep_dir() -> io::Result<PathBuf> {
    Ok(upl_home()?.join("rep"))
}

/// `~/.upl/rep/cred` — credentials store.
pub fn cred_dir() -> io::Result<PathBuf> {
    Ok(rep_dir()?.join("cred"))
}

/// `~/.upl/rep/cert` — TLS cert storage.
pub fn cert_dir() -> io::Result<PathBuf> {
    Ok(rep_dir()?.join("cert"))
}

/// `~/.upl/repconfig` — client configuration (host/port/tls/token/...).
pub fn repconfig_path() -> io::Result<PathBuf> {
    Ok(upl_home()?.join("repconfig"))
}

/// `~/.upl/prompts` — local prompt library (pull destination).
pub fn prompts_dir() -> io::Result<PathBuf> {
    Ok(upl_home()?.join("prompts"))
}

/// `~/.upl/rep/prompts/<username>` — per-user prompt root.
pub fn user_dir(username: &str) -> io::Result<PathBuf> {
    Ok(rep_dir()?.join("prompts").join(username))
}

/// `~/.upl/rep/<username>/<name>` — per-prompt directory.
pub fn prompt_dir(username: &str, name: &str) -> io::Result<PathBuf> {
    Ok(user_dir(username)?.join(name))
}

/// `~/.upl/rep/<username>/<name>/meta.json`
pub fn prompt_meta_path(username: &str, name: &str) -> io::Result<PathBuf> {
    Ok(prompt_dir(username, name)?.join("meta.json"))
}

/// `~/.upl/rep/<username>/<name>/<version>.prompt.txt`
pub fn prompt_version_path(username: &str, name: &str, version: u32) -> io::Result<PathBuf> {
    Ok(prompt_dir(username, name)?.join(format!("{version}.prompt.txt")))
}

/// `~/.upl/rep/cred/<username>`
pub fn cred_file(username: &str) -> io::Result<PathBuf> {
    Ok(cred_dir()?.join(username))
}

// ---------------------------------------------------------------------------
// Visibility
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Private,
}

impl Visibility {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "public" | "pub" => Ok(Visibility::Public),
            "private" | "priv" => Ok(Visibility::Private),
            other => Err(format!("invalid visibility '{other}' (expected public|private)")),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-prompt manifest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMeta {
    pub visibility: Visibility,
    pub latest_version: u32,
    /// SHA-256 of the latest version's content, used to skip version
    /// increments when a push carries identical content.
    #[serde(default)]
    pub latest_sha256: String,
}

impl PromptMeta {
    pub fn load(username: &str, name: &str) -> io::Result<Option<Self>> {
        let p = prompt_meta_path(username, name)?;
        if !p.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&p)?;
        let meta: PromptMeta = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(meta))
    }

    pub fn save(&self, username: &str, name: &str) -> io::Result<()> {
        let p = prompt_meta_path(username, name)?;
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&p, bytes)
    }
}

// ---------------------------------------------------------------------------
// Credentials (argon2id)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// argon2id PHC-encoded hash string.
    pub password_hash: String,
    /// Optional armored GPG public key the user registered for challenge
    /// signing.
    pub gpg_pubkey: Option<String>,
}

impl Credential {
    pub fn load(username: &str) -> io::Result<Option<Self>> {
        let p = cred_file(username)?;
        if !p.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&p)?;
        let cred: Credential = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(cred))
    }

    pub fn save(&self, username: &str) -> io::Result<()> {
        let dir = cred_dir()?;
        fs::create_dir_all(&dir)?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(cred_file(username)?, bytes)
    }

    pub fn delete(username: &str) -> io::Result<()> {
        let p = cred_file(username)?;
        if p.exists() {
            fs::remove_file(&p)?;
        }
        Ok(())
    }

    /// Hash a password with argon2id and return the PHC string.
    pub fn hash_password(password: &str) -> Result<String, String> {
        use argon2::{
            password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
            Argon2,
        };
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| format!("argon2 hash failed: {e}"))
    }

    /// Verify a plaintext password against this credential's hash.
    pub fn verify_password(&self, password: &str) -> bool {
        use argon2::{
            password_hash::{PasswordHash, PasswordVerifier},
            Argon2,
        };
        let Ok(parsed) = PasswordHash::new(&self.password_hash) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }
}

// ---------------------------------------------------------------------------
// Name / content validation
// ---------------------------------------------------------------------------

/// Validate a prompt name. Allowed: lowercase alphanumeric (UTF-8) characters
/// and `_` only. Uppercase letters, `-`, `.` and other punctuation are not
/// permitted. Max 255 chars. This mirrors the `name` field rules in RFC §2.1.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("prompt name is empty".into());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "prompt name too long ({} > {MAX_NAME_LEN})",
            name.len()
        ));
    }
    if !crate::upl::parser::is_valid_name(name) {
        return Err(
            "prompt name may only contain lowercase alphanumeric (UTF-8) characters and '_'".into(),
        );
    }
    Ok(())
}

/// Validate prompt content size and that it parses as a UPL document.
/// Returns the parsed `name` (required) on success.
pub fn validate_prompt_content(content: &[u8]) -> Result<String, String> {
    if content.len() > MAX_PROMPT_BYTES {
        return Err(format!(
            "prompt too large ({} > {MAX_PROMPT_BYTES} bytes)",
            content.len()
        ));
    }
    let text = std::str::from_utf8(content).map_err(|e| format!("prompt is not utf-8: {e}"))?;
    let prompt = PromptParser::parse(text).map_err(|e| format!("invalid prompt: {e}"))?;
    validate_name(&prompt.name)?;
    Ok(prompt.name)
}

// ---------------------------------------------------------------------------
// Wire protocol
// ---------------------------------------------------------------------------

/// A request sent by the client over the (optionally TLS-secured) connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// First leg of login: ask for a challenge nonce.
    LoginStart { username: String },
    /// Second leg of login: prove knowledge of the password (and optionally
    /// sign the challenge with the user's GPG private key).
    LoginFinish {
        password: String,
        gpg_signature: Option<Vec<u8>>,
    },
    /// Push a new prompt version. `name` MUST equal the parsed `name` field.
    Push {
        token: String,
        name: String,
        visibility: Visibility,
        content: Vec<u8>,
    },
    /// Pull a prompt. `token` is optional: anonymous pull is allowed for
    /// public prompts. `version = None` means latest.
    Pull {
        username: String,
        name: String,
        version: Option<u32>,
        token: Option<String>,
    },
    /// Delete all versions of a prompt owned by the authenticated user.
    Delete { token: String, name: String },
}

/// A response sent by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// Sent in reply to `LoginStart`.
    Challenge {
        nonce: Vec<u8>,
        requires_gpg: bool,
    },
    /// Sent in reply to `LoginFinish` on success.
    LoginOk { token: String },
    /// Sent in reply to `Push` on success.
    PushOk { version: u32 },
    /// Sent in reply to `Pull` on success.
    PullOk { version: u32, content: Vec<u8> },
    /// Generic success.
    Ok,
    /// An error occurred. `code` is a coarse category (see `ErrCode`).
    Error { code: u16, message: String },
}

/// Coarse error categories used as `Response::Error.code`.
pub mod err_code {
    pub const BAD_REQUEST: u16 = 1;
    pub const UNAUTHORIZED: u16 = 2;
    pub const FORBIDDEN: u16 = 3;
    pub const NOT_FOUND: u16 = 4;
    pub const CONFLICT: u16 = 5;
    pub const INVALID_PROMPT: u16 = 6;
    pub const INTERNAL: u16 = 7;
}

impl Response {
    pub fn err(code: u16, message: impl Into<String>) -> Self {
        Response::Error {
            code,
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Framing: 4-byte big-endian length prefix + bincode payload.
// ---------------------------------------------------------------------------

/// Write a length-prefixed bincode message.
pub fn write_msg<W: Write>(w: &mut W, msg: &impl Serialize) -> io::Result<()> {
    let bytes = bincode::serialize(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = bytes.len() as u32;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Read a length-prefixed bincode message.
pub fn read_msg<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    // Cap allocations to avoid DoS via huge length prefixes.
    if len > 64 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    bincode::deserialize(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Generate a random hex token of `TOKEN_LEN` bytes (64 hex chars).
pub fn random_token() -> String {
    use rand::RngCore;
    let mut buf = [0u8; TOKEN_LEN];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Generate a random nonce of the given length.
pub fn random_nonce(len: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut buf = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}
