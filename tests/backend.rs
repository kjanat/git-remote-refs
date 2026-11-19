use git_remote_refs::{
    Capabilities, Discovery, Error, Kind, Limits, Object, ObjectFormat, ObjectId, RefList, Remote,
    Request, Resolution, Response, parse_discovery, parse_fetch, parse_ls_refs, resolve,
};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};

const SCRIPT: &str = include_str!("fixture.sh");

fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
        let lock = File::create(tmp.join("fixtures.lock")).unwrap();
        lock.lock().unwrap();
        let root = tmp.join("fixtures");
        let marker = root.join(".script");
        if fs::read_to_string(&marker).ok().as_deref() != Some(SCRIPT) {
            let status = Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixture.sh"))
                .arg(&root)
                .status()
                .unwrap();
            assert!(status.success());
            fs::write(&marker, SCRIPT).unwrap();
        }
        root
    })
}

struct Reply {
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    fn response(&self) -> Response<'_> {
        Response {
            status: self.status,
            content_type: self.content_type.as_deref(),
            body: &self.body,
        }
    }
}

fn git() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

fn exchange(request: &Request, v2: bool) -> Reply {
    let rest = request.url.strip_prefix("http://fixtures").unwrap();
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut command = git();
    command
        .arg("http-backend")
        .env("GIT_PROJECT_ROOT", root())
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", request.method.as_str())
        .env("CONTENT_LENGTH", request.body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    for (name, value) in &request.headers {
        match *name {
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
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&request.body)
        .unwrap();
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

fn remote(name: &str) -> Remote {
    Remote::new(&format!("http://fixtures/{name}.git")).unwrap()
}

fn discover(name: &str, v2: bool) -> Result<Discovery, Error> {
    parse_discovery(&exchange(&remote(name).discovery_request(), v2).response())
}

fn capabilities(name: &str) -> Capabilities {
    match discover(name, true).unwrap() {
        Discovery::V2(capabilities) => capabilities,
        Discovery::V0(_) => panic!("expected protocol v2"),
    }
}

fn list(name: &str, prefixes: &[&str]) -> RefList {
    let capabilities = capabilities(name);
    let request = remote(name)
        .ls_refs_request(&capabilities, prefixes)
        .unwrap();
    parse_ls_refs(
        &capabilities,
        &exchange(&request, true).response(),
        prefixes,
    )
    .unwrap()
}

fn fetch(name: &str, wants: &[ObjectId]) -> Result<BTreeMap<ObjectId, Object>, Error> {
    let capabilities = capabilities(name);
    let request = remote(name).fetch_request(&capabilities, wants)?;
    parse_fetch(
        &capabilities,
        &exchange(&request, true).response(),
        wants,
        &Limits::default(),
    )
}

fn rev(name: &str, revision: &str) -> ObjectId {
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
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn v2_listing_reports_peeled_tags_and_symrefs() {
    let refs = list("sha1", &[]);
    assert!(refs.is_complete());
    assert_eq!(refs.format, ObjectFormat::Sha1);
    let head = refs.get("HEAD").unwrap();
    assert_eq!(head.symref_target.as_deref(), Some("refs/heads/main"));
    assert_eq!(head.oid, rev("sha1", "main"));
    let annotated = refs.get("refs/tags/annotated").unwrap();
    assert_eq!(annotated.oid, rev("sha1", "annotated"));
    assert_eq!(annotated.peeled, Some(rev("sha1", "main")));
    assert_eq!(
        refs.get("refs/tags/nested").unwrap().peeled,
        Some(rev("sha1", "main"))
    );
    assert_eq!(
        refs.get("refs/tags/tree-tag").unwrap().peeled,
        Some(rev("sha1", "main^{tree}"))
    );
    assert_eq!(refs.get("refs/tags/light").unwrap().peeled, None);
    assert_eq!(refs.refs.len(), 11);
}

#[test]
fn v0_advertisement_matches_v2_listing() {
    let Discovery::V0(v0) = discover("sha1", false).unwrap() else {
        panic!("expected protocol v0");
    };
    assert_eq!(v0, list("sha1", &[]));
    let Discovery::V0(sha256) = discover("sha256", false).unwrap() else {
        panic!("expected protocol v0");
    };
    assert_eq!(sha256, list("sha256", &[]));
}

#[test]
fn prefixes_narrow_listing_and_coverage() {
    let refs = list("sha1", &["refs/tags/v2", "refs/heads/v2"]);
    assert_eq!(
        refs.refs.keys().collect::<Vec<_>>(),
        ["refs/heads/v2", "refs/tags/v2"]
    );
    assert!(refs.covers("refs/tags/v2.1"));
    assert!(!refs.covers("refs/heads/main"));
    let capabilities = capabilities("sha1");
    assert_eq!(
        remote("sha1").ls_refs_request(&capabilities, &["refs/tags/\nx"]),
        Err(Error::InvalidPrefix)
    );
}

#[test]
fn empty_repository_lists_unborn_head() {
    let refs = list("empty", &[]);
    assert!(refs.refs.is_empty());
    assert_eq!(
        refs.unborn.get("HEAD").map(String::as_str),
        Some("refs/heads/main")
    );
    let Discovery::V0(v0) = discover("empty", false).unwrap() else {
        panic!("expected protocol v0");
    };
    assert!(v0.refs.is_empty());
}

#[test]
fn fetches_a_single_verified_commit() {
    let main = rev("sha1", "main");
    let objects = fetch("sha1", &[main]).unwrap();
    assert_eq!(objects.len(), 1);
    let Object::Commit(commit) = &objects[&main] else {
        panic!("expected commit");
    };
    assert_eq!(commit.tree, rev("sha1", "main^{tree}"));
    assert_eq!(commit.parents, [rev("sha1", "main~1")]);
    assert_eq!(commit.author.name, b"Ada Lovelace");
    assert_eq!(commit.committer.email, b"ada@example.com");
    assert_eq!(commit.committer.time, 1_767_315_845);
    assert_eq!(commit.committer.offset_minutes, 120);
    assert_eq!(commit.message, b"second\n\nwith a body\n");
}

#[test]
fn fetches_tags_trees_and_blobs() {
    let annotated = rev("sha1", "annotated");
    let objects = fetch("sha1", &[annotated]).unwrap();
    let Object::Tag(tag) = &objects[&annotated] else {
        panic!("expected tag");
    };
    assert_eq!(
        (tag.target, tag.target_kind),
        (rev("sha1", "main"), Kind::Commit)
    );
    assert_eq!(tag.name, b"annotated");
    assert_eq!(tag.tagger.as_ref().unwrap().name, b"Ada Lovelace");
    assert!(matches!(
        objects.get(&rev("sha1", "main")),
        Some(Object::Commit(_))
    ));

    let nested = rev("sha1", "nested");
    let Object::Tag(outer) = &fetch("sha1", &[nested]).unwrap()[&nested] else {
        panic!("expected tag");
    };
    assert_eq!((outer.target, outer.target_kind), (annotated, Kind::Tag));

    let tree = rev("sha1", "main^{tree}");
    let Object::Tree(entries) = &fetch("sha1", &[tree]).unwrap()[&tree] else {
        panic!("expected tree");
    };
    let names: Vec<&[u8]> = entries.iter().map(|entry| entry.name.as_slice()).collect();
    assert_eq!(
        names,
        [&b"empty"[..], b"file", b"numbers", b"numbers-changed"]
    );

    let file = rev("sha1", "main:file");
    let empty = rev("sha1", "main:empty");
    let blobs = fetch("sha1", &[file, empty]).unwrap();
    assert_eq!(blobs[&file], Object::Blob(b"one\n".to_vec()));
    assert_eq!(blobs[&empty], Object::Blob(Vec::new()));
}

#[test]
fn fetches_from_sha256_repositories() {
    let capabilities = capabilities("sha256");
    assert_eq!(capabilities.object_format(), ObjectFormat::Sha256);
    let main = rev("sha256", "main");
    let Object::Commit(commit) = &fetch("sha256", &[main]).unwrap()[&main] else {
        panic!("expected commit");
    };
    assert_eq!(commit.tree, rev("sha256", "main^{tree}"));
    assert_eq!(
        remote("sha256").fetch_request(&capabilities, &[rev("sha1", "main")]),
        Err(Error::UnsupportedObjectFormat)
    );
}

#[test]
fn servers_without_filter_send_the_whole_snapshot() {
    let capabilities = capabilities("nofilter");
    assert!(!capabilities.supports("fetch", "filter"));
    let main = rev("nofilter", "main");
    let objects = fetch("nofilter", &[main]).unwrap();
    assert!(matches!(objects[&main], Object::Commit(_)));
    assert_eq!(
        objects[&rev("nofilter", "main:numbers-changed")].kind(),
        Kind::Blob
    );
    assert_eq!(objects.len(), 6);
}

#[test]
fn unknown_objects_and_repositories_fail_with_typed_errors() {
    let unknown: ObjectId = "1234567890123456789012345678901234567890".parse().unwrap();
    assert!(matches!(fetch("sha1", &[unknown]), Err(Error::Remote(_))));
    assert_eq!(discover("missing", true), Err(Error::NotFound));
    let Discovery::V0(_) = discover("sha1", false).unwrap() else {
        panic!("expected protocol v0");
    };
}

#[test]
fn resolves_names_in_git_order() {
    let refs = list("sha1", &[]);
    let Resolution::Ref {
        name,
        target,
        others,
    } = resolve(&refs, "v2").unwrap()
    else {
        panic!("expected ref");
    };
    assert_eq!(name, "refs/tags/v2");
    assert_eq!(target.oid, rev("sha1", "refs/tags/v2"));
    assert_eq!(others, ["refs/heads/v2"]);
    let Resolution::Ref { name, others, .. } = resolve(&refs, "heads/x").unwrap() else {
        panic!("expected ref");
    };
    assert_eq!(
        (name.as_str(), others),
        ("refs/heads/x", vec![String::from("refs/heads/heads/x")])
    );
    let Resolution::Ref { name, .. } = resolve(&refs, "refs/heads/heads/x").unwrap() else {
        panic!("expected ref");
    };
    assert_eq!(name, "refs/heads/heads/x");
    let main = rev("sha1", "main");
    assert_eq!(
        resolve(&refs, &main.to_hex()),
        Ok(Resolution::Object {
            oid: main,
            refs: Vec::new(),
        })
    );
    assert_eq!(resolve(&refs, "missing"), Ok(Resolution::NotFound));
    let partial = list("sha1", &["refs/heads/"]);
    assert!(matches!(
        resolve(&partial, "v2"),
        Ok(Resolution::Incomplete { .. })
    ));
}

#[test]
fn keeps_non_utf8_bytes_and_commit_encoding() {
    let main = rev("latin1", "main");
    let Object::Commit(commit) = &fetch("latin1", &[main]).unwrap()[&main] else {
        panic!("expected commit");
    };
    assert_eq!(commit.encoding.as_deref(), Some(&b"ISO-8859-1"[..]));
    assert_eq!(commit.message, b"Caf\xe9\n");
    assert_eq!(commit.author.name, b"\xc9mile");
    let tree = commit.tree;
    let Object::Tree(entries) = &fetch("latin1", &[tree]).unwrap()[&tree] else {
        panic!("expected tree");
    };
    assert_eq!(entries[0].name, b"caf\xe9");
}
