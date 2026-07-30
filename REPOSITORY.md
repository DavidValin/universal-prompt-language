# upl Repository

The `upl` binary serves three roles from a single executable:

1. **Local prompt tooling** — build/browse UPL prompts (`upl build`).
2. **Repository server** — store and serve prompts over TCP+TLS (`upl start_repository`).
3. **Repository client** — push, pull, delete, and manage prompts on a remote server (`upl login`, `push`, `pull`, `del`, ...).

This document describes the repository features (roles 2 and 3).

---

## 1. Layout

All repository state lives under `~/.upl/`:

```
~/.upl/
├── prompts/                      # local prompt library
│   ├── <name>.txt                # locally-authored prompts (repository = "none")
│   └── <host>/                   # pulled prompts, namespaced by host+user
│       └── <user>/
│           └── <name>.txt
├── repconfig                      # client configuration (JSON)
└── rep/                           # server data root
    ├── cert/
    │   ├── server.pem             # TLS cert + private key (copied on start_repository)
    │   └── ca.pem                 # optional pinned CA for client verification
    ├── cred/                      # credentials store
    │   └── <username>             # argon2id hash + optional GPG public key (JSON)
    └── prompts/                   # prompt storage
        └── <username>/            # per-user prompt namespace
            └── <prompt_name>/
                ├── meta.json      # { visibility, latest_version, latest_sha256 }
                ├── 1.prompt.txt   # version 1
                ├── 2.prompt.txt   # version 2
                └── ...
```

### 1.1 Client configuration (`~/.upl/repconfig`)

JSON document written by `set-rep` and `login`:

| Field      | Type             | Description                                              |
|------------|------------------|---------------------------------------------------------|
| `host`     | string           | Repository endpoint, e.g. `host:port`.                  |
| `tls`      | bool             | Whether to use TLS when connecting.                     |
| `gpg_key`  | string?          | Key id/fingerprint used to sign login challenges.        |
| `username` | string?          | Last logged-in username.                                |
| `token`    | string?          | Last session token (set by `login`, sent on `push`/`del`). |

---

## 2. Authentication

### 2.1 User accounts

Accounts are created and removed **locally on the server host** (not over the network) by an administrator:

```
upl register_user [--user <name>] [--password <pw>] [--gpg-key <file|id>]
upl delete_user <username>
```

When run without flags in a TTY, `register_user` prompts interactively. Passwords are hashed with **argon2id** (random salt, PHC-encoded) and stored at `~/.upl/rep/cred/<username>`. Deleting a user also removes their prompt directory.

### 2.2 Login (challenge-response)

`upl login` authenticates against the configured repository and stores the resulting session token in `~/.upl/repconfig`. The flow runs on a **single TCP connection**:

1. Client sends `LoginStart { username }`.
2. Server generates a 32-byte random nonce, stores it in per-connection state, and replies with `Challenge { nonce, requires_gpg }`. `requires_gpg` is `true` if the user registered a GPG public key.
3. Client sends `LoginFinish { password, gpg_signature }`:
   - The password is verified against the stored argon2id hash.
   - If the user registered a GPG key, a valid detached signature of the nonce is **required**; the server verifies it against the stored public key using a throwaway GNUPGHOME (the server's real keyring is never touched).
4. On success the server replies `LoginOk { token }` (32 random bytes, hex-encoded) and records the `token → username` mapping in memory. Tokens live for the lifetime of the server process.

Anonymous access is allowed only for pulling **public** prompts (see §4.2).

### 2.3 GPG signing (client side)

When `set-rep` was given a `gpg_key` (a key id/fingerprint) and the server demands a signature, `login` signs the nonce with the user's GPG private key by invoking:

```
gpg --batch --yes --detach-sign --local-user <key_id> --output -
```

and piping the nonce to gpg's stdin. The resulting binary signature is sent in `LoginFinish`.

---

## 3. Wire protocol

All client-server communication uses **length-prefixed bincode**:

```
[ 4-byte big-endian length N ][ N bytes of bincode payload ]
```

Messages larger than 64 MiB are rejected by the framing layer.

### 3.1 Request

```rust
enum Request {
    LoginStart  { username: String },
    LoginFinish { password: String, gpg_signature: Option<Vec<u8>> },
    Push   { token: String, name: String, visibility: Visibility, content: Vec<u8> },
    Pull   { username: String, name: String, version: Option<u32>, token: Option<String> },
    Delete { token: String, name: String },
}
```

### 3.2 Response

```rust
enum Response {
    Challenge { nonce: Vec<u8>, requires_gpg: bool },
    LoginOk   { token: String },
    PushOk    { version: u32 },
    PullOk    { version: u32, content: Vec<u8> },
    Ok,
    Error     { code: u16, message: String },
}
```

### 3.3 Error codes

| Code | Meaning           | Typical cause                                    |
|------|-------------------|--------------------------------------------------|
| 1    | `BAD_REQUEST`     | Malformed name, missing fields.                 |
| 2    | `UNAUTHORIZED`    | Bad/expired token, wrong password, missing GPG. |
| 3    | `FORBIDDEN`       | Anonymous pull of a private prompt.            |
| 4    | `NOT_FOUND`       | Prompt or version does not exist.              |
| 5    | `CONFLICT`        | (Reserved for future use.)                      |
| 6    | `INVALID_PROMPT`  | Content fails UPL parsing or name/file mismatch. |
| 7    | `INTERNAL`        | Server-side I/O or storage failure.            |

A connection stays open across multiple request/response pairs until the client disconnects. The login challenge nonce is scoped to a single connection, so `LoginStart` and `LoginFinish` must use the same connection.

---

## 4. Commands

### 4.1 Server

#### `upl start_repository [tls_cert] [bind_addr]`

Starts the repository server. Behavior depends on whether a TLS cert is provided:

- **With `tls_cert`**: the PEM file (containing the certificate chain **and** the private key) is copied into `~/.upl/rep/cert/server.pem` and the server speaks TLS.
- **Without `tls_cert`**: the server runs in **plain TCP** mode (useful for local testing).

`bind_addr` defaults to the `host` field in `~/.upl/repconfig`, or `0.0.0.0:7654` if unset. Each accepted connection is handled on its own thread.

> Note: TLS cert verification on the client side uses `~/.upl/rep/cert/ca.pem` if present. Without it, the client accepts the server's cert unverified and prints a warning.

### 4.2 Client

#### `upl set-rep <host:port> [--tls] [gpg_key_file]`

Writes `~/.upl/repconfig`. `--tls` enables TLS for all subsequent client operations. The optional `gpg_key_file` is read and its contents (a key id/fingerprint) stored as `gpg_key` for use at `login`.

#### `upl get-rep`

Prints the configured repository endpoint, TLS flag, GPG key, username, and whether a session token is set.

#### `upl login [--user <name>] [--password <pw>]`

Authenticates (see §2.2) and stores the token in `~/.upl/repconfig`. Without flags, prompts interactively (only when stdin is a TTY).

#### `upl push <local_prompt.upl|txt> [--visibility public|private]`

Reads the file, parses it to extract its `name`, and pushes the raw file bytes under that name. The file MUST use the `.txt` or `.upl` extension and its base name MUST equal the parsed `name` (RFC §2). The server re-validates the content and rejects the push if:

- the content is not a valid UPL document,
- the parsed `name` does not match the provided `name`,
- the name exceeds 255 characters or contains characters other than lowercase alphanumeric (UTF-8) characters and `_`,
- the file exceeds 20 MB.

Visibility defaults to **private** when `--visibility` is omitted. A successful push returns the new integer version number. **Deduplication**: the server computes the SHA-256 of the incoming content and compares it to the latest version's hash. If identical, no new version is created — the current version number is returned and only the visibility is updated if it changed. Versions auto-increment (only when content differs) starting from 1; concurrent pushes to the same `(user, name)` are serialized with a per-name lock so version numbers never collide.

#### `upl pull <username>/<prompt_name>[/<version>]`

Fetches a prompt. When `version` is omitted, the latest version is returned.

- **Public** prompts can be pulled **anonymously** (no login required).
- **Private** prompts require a valid session token belonging to the owner.

The result is written to `~/.upl/prompts/<host>/<username>/<name>.txt`, namespaced by host and user so prompts from different repositories or authors never collide. A `source:` metadata field is injected immediately after the opening `--` delimiter, set to `<host>/<username>/<prompt_name>`, so pulled prompts carry their provenance.

### 4.3 Prompt browser

The `upl build` TUI browser scans `~/.upl/prompts` **recursively**, so both locally-authored prompts (top-level files, shown with `repository = "none"`) and pulled prompts (nested under `<host>/<user>/`) appear in the list. A **REPOSITORY** column shows the originating host for each prompt.

#### `upl del <prompt_name>`

Deletes **all versions** of a prompt in the authenticated user's own account. Requires a valid session token. The entire `~/.upl/rep/prompts/<user>/<name>/` directory is removed.

---

## 5. Visibility and access control

| Operation | Public prompt            | Private prompt                        |
|-----------|--------------------------|---------------------------------------|
| Pull      | Anyone (anonymous OK)    | Owner only (valid token required)     |
| Push      | Owner only               | Owner only                            |
| Delete    | Owner only               | Owner only                            |

Visibility is recorded per prompt in `meta.json` and is set on every push — pushing the same name with a different visibility updates it, even when the content is unchanged (the version is not incremented but the visibility field is updated).

---

## 6. Versioning

- Versions are monotonically increasing integers starting at 1.
- `meta.json` tracks `latest_version` and `latest_sha256`, and is updated atomically under a per-`(user, name)` lock, so concurrent pushes produce sequential versions without gaps or duplicates.
- **Deduplication**: before creating a new version, the server computes the SHA-256 of the incoming content and compares it to `latest_sha256`. If they match, no new version file is written and the current `latest_version` is returned — only the `visibility` is updated if it changed. A new version is created only when the content differs.
- `pull` with no version returns `latest_version`; `pull <name>/<v>` returns that exact version, or `NOT_FOUND` if `v` is 0 or greater than `latest_version`.

---

## 7. Limits and validation

| Limit                  | Value      | Enforced by               |
|------------------------|------------|---------------------------|
| Prompt name length     | ≤ 255      | `validate_name`           |
| Prompt name charset    | lowercase alphanumeric (UTF-8) + `_` | `validate_name`     |
| Prompt content size    | ≤ 20 MB    | `validate_prompt_content` |
| Wire message size      | ≤ 64 MB    | `read_msg` framing        |
| Required metadata      | `name` field | UPL parser + server        |

The push name must equal the prompt's parsed `name` and the file's base name; mismatches are rejected with `INVALID_PROMPT`.

---

## 8. TLS

- The server loads its cert chain and private key from `~/.upl/rep/cert/server.pem` (copied from the path passed to `start_repository`).
- The client verifies the server against `~/.upl/rep/cert/ca.pem` if present; otherwise it accepts the cert unverified and prints a warning. To pin a CA, copy it to that path (e.g. `cp ca.pem ~/.upl/rep/cert/ca.pem`).
- Client-side TLS is enabled per-repository via `set-rep --tls`.

---

## 9. CLI summary

```
upl build [<folder> | <prompt.upl|txt>] [--no-input]
upl login   [--user <name>] [--password <pw>]
upl push    <prompt.upl|txt> [--visibility public|private]
upl pull    <username>/<prompt_name>[/<version>]
upl del     <prompt_name>
upl set-rep <host:port> [--tls] [gpg_key_file]
upl get-rep
upl start_repository [tls_cert] [bind_addr]
upl register_user [--user <name>] [--password <pw>] [--gpg-key <file|id>]
upl delete_user <username>
```

Non-interactive flags (`--user`, `--password`, `--gpg-key`) make `login` and `register_user` scriptable; in a TTY without them, `inquire`-based prompts are used.
