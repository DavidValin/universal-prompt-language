// Integration tests for the repository server + client over plain TCP.
//
// These tests boot `server::serve` on a kernel-chosen port
// (TcpListener bound to 127.0.0.1:0) inside the test process, then talk to
// it directly via the `rep_protocol` wire frames. A temporary HOME is used
// so the real `~/.upl` is never touched.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Once;

use universal_prompt_language::repository::protocol::{
    self, Credential, PromptMeta, Request, Response, Visibility, read_msg, write_msg,
};
use universal_prompt_language::repository::server;

static INIT: Once = Once::new();

/// Set `HOME` to a temp dir and create the rep/cred dirs. Runs once per
/// process (the first test to call it wins; subsequent calls are no-ops).
fn setup_home() {
    INIT.call_once(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("upl-rep-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HOME", &dir);
        fs::create_dir_all(protocol::cred_dir().unwrap()).unwrap();
        fs::create_dir_all(protocol::rep_dir().unwrap()).unwrap();
    });
}

/// A connected client stream that speaks the wire protocol.
struct Client {
    sock: TcpStream,
}

impl Client {
    fn new(addr: &str) -> Self {
        let sock = TcpStream::connect(addr).unwrap();
        sock.set_nodelay(true).unwrap();
        Client { sock }
    }

    fn call(&mut self, req: &Request) -> Response {
        write_msg(&mut self.sock, req).unwrap();
        read_msg::<_, Response>(&mut self.sock).unwrap()
    }
}

/// Boot the server in-process on a random port, returning the bound address.
fn boot_server() -> String {
    setup_home();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let _ = server::serve(listener, None);
    });
    addr
}

fn register_user(user: &str, password: &str) {
    setup_home();
    let hash = Credential::hash_password(password).unwrap();
    let cred = Credential {
        password_hash: hash,
        gpg_pubkey: None,
    };
    cred.save(user).unwrap();
}

fn login(client: &mut Client, user: &str, password: &str) -> String {
    let resp = client.call(&Request::LoginStart {
        username: user.to_string(),
    });
    let nonce = match resp {
        Response::Challenge { nonce, .. } => nonce,
        _ => panic!("expected Challenge, got {resp:?}"),
    };
    assert!(!nonce.is_empty());
    let resp = client.call(&Request::LoginFinish {
        password: password.to_string(),
        gpg_signature: None,
    });
    match resp {
        Response::LoginOk { token } => token,
        _ => panic!("expected LoginOk, got {resp:?}"),
    }
}

const SAMPLE: &str = "\
--
name: hello
title: Hello
params:
  name:
    type: string
    def: \"world\"
--
Hello, [[[NAME]]]!
--
";

const SAMPLE_V2: &str = "\
--
name: hello
title: Hello v2
params:
  name:
    type: string
    def: \"world\"
--
Hello again, [[[NAME]]]!
--
";

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn login_with_correct_password_succeeds() {
    let addr = boot_server();
    register_user("alice", "s3cret");
    let mut client = Client::new(&addr);
    let token = login(&mut client, "alice", "s3cret");
    assert!(!token.is_empty());
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn login_with_wrong_password_fails() {
    let addr = boot_server();
    register_user("bob", "correct-horse");
    let mut client = Client::new(&addr);
    client.call(&Request::LoginStart {
        username: "bob".to_string(),
    });
    let resp = client.call(&Request::LoginFinish {
        password: "wrong".to_string(),
        gpg_signature: None,
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::UNAUTHORIZED));
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn push_and_pull_roundtrip() {
    let addr = boot_server();
    register_user("carol", "pw");
    let mut client = Client::new(&addr);
    let token = login(&mut client, "carol", "pw");

    // Push version 1 (private).
    let resp = client.call(&Request::Push {
        token: token.clone(),
        name: "hello".to_string(),
        visibility: Visibility::Private,
        content: SAMPLE.as_bytes().to_vec(),
    });
    match resp {
        Response::PushOk { version } => assert_eq!(version, 1),
        _ => panic!("expected PushOk, got {resp:?}"),
    }

    // Pushing identical content again should NOT create version 2 — it
    // returns the same version 1 (dedup by SHA-256).
    let resp = client.call(&Request::Push {
        token: token.clone(),
        name: "hello".to_string(),
        visibility: Visibility::Public,
        content: SAMPLE.as_bytes().to_vec(),
    });
    match resp {
        Response::PushOk { version } => assert_eq!(version, 1),
        _ => panic!("expected PushOk (dedup), got {resp:?}"),
    }

    // Push different content (public) — now version 2.
    let resp = client.call(&Request::Push {
        token: token.clone(),
        name: "hello".to_string(),
        visibility: Visibility::Public,
        content: SAMPLE_V2.as_bytes().to_vec(),
    });
    match resp {
        Response::PushOk { version } => assert_eq!(version, 2),
        _ => panic!("expected PushOk v2, got {resp:?}"),
    }

    // Anonymous pull of latest (public) returns version 2.
    let resp = client.call(&Request::Pull {
        username: "carol".to_string(),
        name: "hello".to_string(),
        version: None,
        token: None,
    });
    match resp {
        Response::PullOk { version, content } => {
            assert_eq!(version, 2);
            assert_eq!(content, SAMPLE_V2.as_bytes());
        }
        _ => panic!("expected PullOk, got {resp:?}"),
    }

    // Anonymous pull of version 1 (original content).
    let resp = client.call(&Request::Pull {
        username: "carol".to_string(),
        name: "hello".to_string(),
        version: Some(1),
        token: None,
    });
    match resp {
        Response::PullOk { version, content } => {
            assert_eq!(version, 1);
            assert_eq!(content, SAMPLE.as_bytes());
        }
        _ => panic!("expected PullOk, got {resp:?}"),
    }
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn anonymous_pull_of_private_prompt_is_forbidden() {
    let addr = boot_server();
    register_user("dave", "pw");
    let mut client = Client::new(&addr);
    let token = login(&mut client, "dave", "pw");
    client.call(&Request::Push {
        token,
        name: "hello".to_string(),
        visibility: Visibility::Private,
        content: SAMPLE.as_bytes().to_vec(),
    });
    // Anonymous pull should be forbidden.
    let resp = client.call(&Request::Pull {
        username: "dave".to_string(),
        name: "hello".to_string(),
        version: None,
        token: None,
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::FORBIDDEN));
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn push_rejects_invalid_prompt() {
    let addr = boot_server();
    register_user("eve", "pw");
    let mut client = Client::new(&addr);
    let token = login(&mut client, "eve", "pw");
    let resp = client.call(&Request::Push {
        token,
        name: "broken".to_string(),
        visibility: Visibility::Private,
        content: b"this is not a valid UPL file".to_vec(),
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::INVALID_PROMPT));
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn push_rejects_name_id_mismatch() {
    let addr = boot_server();
    register_user("frank", "pw");
    let mut client = Client::new(&addr);
    let token = login(&mut client, "frank", "pw");
    let resp = client.call(&Request::Push {
        token,
        name: "different_name".to_string(),
        visibility: Visibility::Private,
        content: SAMPLE.as_bytes().to_vec(),
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::INVALID_PROMPT));
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn pull_nonexistent_returns_not_found() {
    let addr = boot_server();
    let mut client = Client::new(&addr);
    let resp = client.call(&Request::Pull {
        username: "ghost".to_string(),
        name: "nope".to_string(),
        version: None,
        token: None,
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::NOT_FOUND));
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn delete_removes_all_versions() {
    let addr = boot_server();
    register_user("grace", "pw");
    let mut client = Client::new(&addr);
    let token = login(&mut client, "grace", "pw");
    client.call(&Request::Push {
        token: token.clone(),
        name: "hello".to_string(),
        visibility: Visibility::Public,
        content: SAMPLE.as_bytes().to_vec(),
    });
    client.call(&Request::Push {
        token: token.clone(),
        name: "hello".to_string(),
        visibility: Visibility::Public,
        content: SAMPLE_V2.as_bytes().to_vec(),
    });
    // Verify meta exists.
    let meta = PromptMeta::load("grace", "hello").unwrap().unwrap();
    assert_eq!(meta.latest_version, 2);

    // Delete.
    let resp = client.call(&Request::Delete {
        token: token.clone(),
        name: "hello".to_string(),
    });
    assert!(matches!(resp, Response::Ok));

    // Pull should now 404.
    let resp = client.call(&Request::Pull {
        username: "grace".to_string(),
        name: "hello".to_string(),
        version: None,
        token: None,
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::NOT_FOUND));
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn delete_without_token_is_unauthorized() {
    let addr = boot_server();
    let mut client = Client::new(&addr);
    let resp = client.call(&Request::Delete {
        token: "bogus".to_string(),
        name: "hello".to_string(),
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::UNAUTHORIZED));
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn pull_with_invalid_version_returns_not_found() {
    let addr = boot_server();
    register_user("heidi", "pw");
    let mut client = Client::new(&addr);
    let token = login(&mut client, "heidi", "pw");
    client.call(&Request::Push {
        token,
        name: "hello".to_string(),
        visibility: Visibility::Public,
        content: SAMPLE.as_bytes().to_vec(),
    });
    let resp = client.call(&Request::Pull {
        username: "heidi".to_string(),
        name: "hello".to_string(),
        version: Some(99),
        token: None,
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::NOT_FOUND));
}

#[test]
#[ignore = "requires TCP loopback (run with --ignored)"]
fn invalid_token_for_push_is_unauthorized() {
    let addr = boot_server();
    let mut client = Client::new(&addr);
    let resp = client.call(&Request::Push {
        token: "nope".to_string(),
        name: "hello".to_string(),
        visibility: Visibility::Private,
        content: SAMPLE.as_bytes().to_vec(),
    });
    assert!(matches!(resp, Response::Error { code, .. } if code == protocol::err_code::UNAUTHORIZED));
}
