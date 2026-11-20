use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};

const SCRIPT: &str = include_str!("../fixture.sh");

pub fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
        let lock = File::create(tmp.join("fixtures.lock")).unwrap();
        lock.lock().unwrap();
        let root = tmp.join("fixtures");
        let marker = root.join(".script");
        if fs::read_to_string(&marker).ok().as_deref() != Some(SCRIPT) {
            let status = Command::new("bash")
                .args(["-c", SCRIPT, "fixture.sh"])
                .arg(&root)
                .status()
                .unwrap();
            assert!(status.success());
            fs::write(&marker, SCRIPT).unwrap();
        }
        root
    })
}

pub struct Reply {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

pub fn git() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

pub fn exchange<'a>(
    method: &str,
    url: &str,
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    body: &[u8],
    v2: bool,
) -> Reply {
    let rest = url.strip_prefix("http://fixtures").unwrap();
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut command = git();
    command
        .arg("http-backend")
        .env("GIT_PROJECT_ROOT", root())
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", method)
        .env("CONTENT_LENGTH", body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    for (name, value) in headers {
        match name {
            "Content-Type" => {
                command.env("CONTENT_TYPE", value);
            }
            "Git-Protocol" if v2 => {
                command.env("HTTP_GIT_PROTOCOL", value);
            }
            _ => {}
        }
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(body).unwrap();
    let output = child.wait_with_output().unwrap();
    let split = output
        .stdout
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8(output.stdout[..split].to_vec()).unwrap();
    let mut reply = Reply {
        status: 200,
        content_type: None,
        body: output.stdout[split + 4..].to_vec(),
    };
    for line in head.split("\r\n") {
        let (name, value) = line.split_once(": ").unwrap();
        match name.to_ascii_lowercase().as_str() {
            "status" => reply.status = value[..3].parse().unwrap(),
            "content-type" => reply.content_type = Some(value.into()),
            _ => {}
        }
    }
    reply
}

pub fn rev(name: &str, revision: &str) -> String {
    let output = git()
        .args([
            "-C",
            &root().join(format!("{name}.git")).to_string_lossy(),
            "rev-parse",
            revision,
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{revision}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}
