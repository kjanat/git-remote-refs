use exports::kjanat::git_remote_refs::git::{
    Blob, Error, Fetched, GitObject, GitRef, Method, ObjectFormat, ObjectKind, ObjectMatch,
    RefList, RefMatch, Request, Resolution, Response,
};
use std::{path::Path, sync::OnceLock};
use wasmtime::{
    Engine, Store,
    component::{Component, Linker, ResourceAny},
};

#[path = "../../../tests/support/mod.rs"]
mod support;

wasmtime::component::bindgen!({
    world: "git-remote-refs",
    additional_derives: [PartialEq],
});

const COMPONENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../go/git-remote-refs.wasm");

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(Engine::default)
}

fn component() -> &'static Component {
    static COMPILED: OnceLock<Component> = OnceLock::new();
    COMPILED.get_or_init(|| {
        let path = Path::new(COMPONENT);
        assert!(
            path.exists(),
            "{} is missing; run `mise run component:build`",
            path.display()
        );
        Component::from_file(engine(), path).unwrap()
    })
}

struct Host {
    store: Store<()>,
    bindings: GitRemoteRefs,
}

impl Host {
    fn new() -> Self {
        let mut store = Store::new(engine(), ());
        let bindings =
            GitRemoteRefs::instantiate(&mut store, component(), &Linker::new(engine())).unwrap();
        Self { store, bindings }
    }

    fn candidates(&mut self, name: &str) -> Result<Vec<String>, Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.call_candidates(&mut self.store, name).unwrap()
    }

    fn parse_advertisement(&mut self, body: &[u8]) -> Result<RefList, Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.call_parse_advertisement(&mut self.store, body).unwrap()
    }

    fn remote(&mut self, url: &str) -> Result<ResourceAny, Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote().call_constructor(&mut self.store, url).unwrap()
    }

    fn discovery_request(&mut self, remote: ResourceAny) -> Request {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote()
            .call_discovery_request(&mut self.store, remote)
            .unwrap()
    }

    fn accept_discovery(&mut self, remote: ResourceAny, reply: &Response) -> Result<(), Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote()
            .call_accept_discovery(&mut self.store, remote, reply)
            .unwrap()
    }

    fn list_request(
        &mut self,
        remote: ResourceAny,
        prefixes: &[String],
    ) -> Result<Option<Request>, Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote()
            .call_list_request(&mut self.store, remote, prefixes)
            .unwrap()
    }

    fn accept_list(&mut self, remote: ResourceAny, reply: &Response) -> Result<(), Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote()
            .call_accept_list(&mut self.store, remote, reply)
            .unwrap()
    }

    fn refs(&mut self, remote: ResourceAny) -> Result<RefList, Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote().call_refs(&mut self.store, remote).unwrap()
    }

    fn resolve(&mut self, remote: ResourceAny, name: &str) -> Result<Resolution, Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote()
            .call_resolve(&mut self.store, remote, name)
            .unwrap()
    }

    fn fetch_request(&mut self, remote: ResourceAny, oids: &[String]) -> Result<Request, Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote()
            .call_fetch_request(&mut self.store, remote, oids)
            .unwrap()
    }

    fn accept_fetch(
        &mut self,
        remote: ResourceAny,
        reply: &Response,
        oids: &[String],
    ) -> Result<Fetched, Error> {
        let git = self.bindings.kjanat_git_remote_refs_git();
        git.remote()
            .call_accept_fetch(&mut self.store, remote, reply, oids)
            .unwrap()
    }

    fn discover(&mut self, name: &str, v2: bool) -> ResourceAny {
        let remote = self.remote(&url(name)).unwrap();
        let reply = send(&self.discovery_request(remote), v2);
        self.accept_discovery(remote, &reply).unwrap();
        remote
    }

    fn list(&mut self, remote: ResourceAny, prefixes: &[&str], v2: bool) -> RefList {
        let prefixes: Vec<String> = prefixes
            .iter()
            .map(|prefix| String::from(*prefix))
            .collect();
        if let Some(request) = self.list_request(remote, &prefixes).unwrap() {
            self.accept_list(remote, &send(&request, v2)).unwrap();
        }
        self.refs(remote).unwrap()
    }

    fn fetch(&mut self, name: &str, oids: &[String]) -> Result<Fetched, Error> {
        let remote = self.discover(name, true);
        let request = self.fetch_request(remote, oids)?;
        self.accept_fetch(remote, &send(&request, true), oids)
    }
}

fn url(name: &str) -> String {
    format!("http://fixtures/{name}.git")
}

fn send(request: &Request, v2: bool) -> Response {
    let method = match request.method {
        Method::Get => "GET",
        Method::Post => "POST",
    };
    let reply = support::exchange(
        method,
        &request.url,
        request
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        &request.body,
        v2,
    );
    Response {
        status: reply.status,
        content_type: reply.content_type,
        body: reply.body,
    }
}

fn rev(name: &str, revision: &str) -> String {
    support::rev(name, revision)
}

fn get<'a, T>(entries: &'a [(String, T)], name: &str) -> Option<&'a T> {
    entries
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn names<T>(entries: &[(String, T)]) -> Vec<&str> {
    entries.iter().map(|(name, _)| name.as_str()).collect()
}

fn pkt(data: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", data.len() + 4).into_bytes();
    out.extend_from_slice(data);
    out
}

fn packfile(pack: &[u8]) -> Response {
    let mut body = pkt(b"packfile\n");
    body.extend(pkt(&[&[1], pack].concat()));
    body.extend_from_slice(b"0000");
    Response {
        status: 200,
        content_type: Some("application/x-git-upload-pack-result".into()),
        body,
    }
}

#[test]
fn requests_carry_method_headers_and_body() {
    let mut host = Host::new();
    let remote = host.remote(&url("sha1")).unwrap();
    assert_eq!(
        host.discovery_request(remote),
        Request {
            method: Method::Get,
            url: "http://fixtures/sha1.git/info/refs?service=git-upload-pack".into(),
            headers: vec![
                (
                    "Accept".into(),
                    "application/x-git-upload-pack-advertisement".into()
                ),
                ("Git-Protocol".into(), "version=2".into()),
            ],
            body: Vec::new(),
        }
    );
    let remote = host.discover("sha1", true);
    let request = host
        .list_request(remote, &["refs/tags/".into()])
        .unwrap()
        .unwrap();
    assert_eq!(request.method, Method::Post);
    assert_eq!(request.url, "http://fixtures/sha1.git/git-upload-pack");
    assert_eq!(
        request.headers[0],
        (
            "Content-Type".into(),
            "application/x-git-upload-pack-request".into()
        )
    );
    assert!(
        String::from_utf8(request.body)
            .unwrap()
            .contains("ref-prefix refs/tags/\n")
    );
}

#[test]
fn lists_refs_over_protocol_v2() {
    let mut host = Host::new();
    let remote = host.discover("sha1", true);
    let refs = host.list(remote, &[], true);
    assert_eq!(refs.object_format, ObjectFormat::Sha1);
    assert_eq!(refs.prefixes, None);
    assert_eq!(refs.refs.len(), 11);
    assert_eq!(
        get(&refs.refs, "HEAD"),
        Some(&GitRef {
            oid: rev("sha1", "main"),
            peeled: None,
            symref_target: Some("refs/heads/main".into()),
        })
    );
    assert_eq!(
        get(&refs.refs, "refs/tags/annotated"),
        Some(&GitRef {
            oid: rev("sha1", "annotated"),
            peeled: Some(rev("sha1", "main")),
            symref_target: None,
        })
    );
    let tags = host.list(remote, &["refs/tags/v"], true);
    assert_eq!(names(&tags.refs), ["refs/tags/v2"]);
    assert_eq!(tags.prefixes, Some(vec!["refs/tags/v".into()]));
    let empty = host.discover("empty", true);
    let unborn = host.list(empty, &[], true);
    assert!(unborn.refs.is_empty());
    assert_eq!(unborn.unborn, [("HEAD".into(), "refs/heads/main".into())]);
}

#[test]
fn protocol_v0_listing_matches_v2() {
    let mut host = Host::new();
    let v0 = host.discover("sha1", false);
    assert_eq!(host.list_request(v0, &[]).unwrap(), None);
    let v2 = host.discover("sha1", true);
    assert_eq!(host.list(v0, &[], false), host.list(v2, &[], true));
    let heads = host.list(v0, &["refs/heads/"], false);
    assert_eq!(
        names(&heads.refs),
        [
            "refs/heads/heads/x",
            "refs/heads/main",
            "refs/heads/v2",
            "refs/heads/x"
        ]
    );
    assert_eq!(
        host.list_request(v0, &["a b".into()]),
        Err(Error::InvalidPrefix)
    );
    assert_eq!(
        host.list_request(v2, &["a b".into()]),
        Err(Error::InvalidPrefix)
    );
}

#[test]
fn resolves_names_in_git_order() {
    let mut host = Host::new();
    let remote = host.discover("sha1", true);
    host.list(remote, &[], true);
    assert_eq!(
        host.resolve(remote, "v2"),
        Ok(Resolution::FoundRef(RefMatch {
            name: "refs/tags/v2".into(),
            target: GitRef {
                oid: rev("sha1", "refs/tags/v2"),
                peeled: None,
                symref_target: None,
            },
            others: vec!["refs/heads/v2".into()],
        }))
    );
    assert_eq!(
        host.resolve(remote, &rev("sha1", "main")),
        Ok(Resolution::FoundObject(ObjectMatch {
            oid: rev("sha1", "main"),
            refs: Vec::new(),
        }))
    );
    assert_eq!(host.resolve(remote, "missing"), Ok(Resolution::NotFound));
    assert_eq!(host.resolve(remote, "v2^{}"), Err(Error::InvalidRefName));
    host.list(remote, &["refs/heads/"], true);
    let Ok(Resolution::Incomplete(missing)) = host.resolve(remote, "v2") else {
        panic!("expected an incomplete resolution");
    };
    assert!(missing.contains(&"refs/tags/v2".into()));
}

#[test]
fn fetches_commits_tags_trees_and_blobs() {
    let mut host = Host::new();
    let main = rev("sha1", "main");
    let fetched = host.fetch("sha1", &[main.to_uppercase()]).unwrap();
    let [GitObject::Commit(commit)] = fetched.wanted.as_slice() else {
        panic!("expected one commit");
    };
    assert_eq!(commit.oid, main);
    assert_eq!(commit.tree, rev("sha1", "main^{tree}"));
    assert_eq!(commit.parents, [rev("sha1", "main~1")]);
    assert_eq!(
        (
            commit.author.name.as_str(),
            commit.author.name_bytes.as_ref()
        ),
        ("Ada Lovelace", None)
    );
    assert_eq!(commit.committer.email, "ada@example.com");
    assert_eq!(commit.committer.time, 1_767_315_845);
    assert_eq!(commit.committer.offset_minutes, 120);
    assert_eq!(commit.encoding, None);
    assert_eq!(
        (commit.message.as_str(), commit.message_bytes.as_ref()),
        ("second\n\nwith a body\n", None)
    );
    assert_eq!(names(&fetched.objects), [main.as_str()]);

    let annotated = rev("sha1", "annotated");
    let fetched = host
        .fetch("sha1", std::slice::from_ref(&annotated))
        .unwrap();
    let [GitObject::Tag(tag)] = fetched.wanted.as_slice() else {
        panic!("expected one tag");
    };
    assert_eq!(
        (tag.target.as_str(), tag.target_kind),
        (main.as_str(), ObjectKind::Commit)
    );
    assert_eq!(
        (tag.name.as_str(), tag.name_bytes.as_ref()),
        ("annotated", None)
    );
    assert_eq!(tag.message, "annotated\n");
    assert_eq!(tag.tagger.as_ref().unwrap().name, "Ada Lovelace");
    assert!(matches!(
        get(&fetched.objects, &main),
        Some(GitObject::Commit(_))
    ));

    let tree = rev("sha256", "main^{tree}");
    let fetched = host.fetch("sha256", &[tree]).unwrap();
    let [GitObject::Tree(listing)] = fetched.wanted.as_slice() else {
        panic!("expected one tree");
    };
    let entries: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(entries, ["empty", "file", "numbers", "numbers-changed"]);
    assert_eq!(listing.entries[1].mode, 0o100644);
    assert_eq!(listing.entries[1].oid, rev("sha256", "main:file"));

    let file = rev("sha1", "main:file");
    let empty = rev("sha1", "main:empty");
    let fetched = host.fetch("sha1", &[empty.clone(), file.clone()]).unwrap();
    assert_eq!(
        fetched.wanted,
        [
            GitObject::Blob(Blob {
                oid: empty,
                data: Vec::new(),
            }),
            GitObject::Blob(Blob {
                oid: file,
                data: b"one\n".to_vec(),
            }),
        ]
    );
}

#[test]
fn keeps_non_utf8_bytes_and_commit_encoding() {
    let mut host = Host::new();
    let main = rev("latin1", "main");
    let fetched = host.fetch("latin1", &[main]).unwrap();
    let [GitObject::Commit(commit)] = fetched.wanted.as_slice() else {
        panic!("expected one commit");
    };
    assert_eq!(commit.encoding.as_deref(), Some("ISO-8859-1"));
    assert_eq!(commit.message, "Caf\u{fffd}\n");
    assert_eq!(commit.message_bytes.as_deref(), Some(&b"Caf\xe9\n"[..]));
    assert_eq!(commit.author.name_bytes.as_deref(), Some(&b"\xc9mile"[..]));
    assert_eq!(commit.committer.name_bytes, None);
    let fetched = host
        .fetch("latin1", std::slice::from_ref(&commit.tree))
        .unwrap();
    let [GitObject::Tree(tree)] = fetched.wanted.as_slice() else {
        panic!("expected one tree");
    };
    assert_eq!(tree.entries[0].name_bytes.as_deref(), Some(&b"caf\xe9"[..]));
}

#[test]
fn refuses_work_before_discovery() {
    let mut host = Host::new();
    let remote = host.remote(&url("sha1")).unwrap();
    let oid = vec!["1".repeat(40)];
    let reply = packfile(&[]);
    let not_ready: Result<(), Error> = Err(Error::Protocol("discovery has not completed".into()));
    assert_eq!(host.refs(remote).map(|_| ()), not_ready);
    assert_eq!(host.resolve(remote, "main").map(|_| ()), not_ready);
    assert_eq!(host.list_request(remote, &[]).map(|_| ()), not_ready);
    assert_eq!(host.accept_list(remote, &reply), not_ready);
    assert_eq!(host.fetch_request(remote, &oid).map(|_| ()), not_ready);
    assert_eq!(
        host.accept_fetch(remote, &reply, &oid).map(|_| ()),
        not_ready
    );
    assert_eq!(host.fetch_request(remote, &[]), Err(Error::InvalidObjectId));
    assert_eq!(
        host.accept_fetch(remote, &reply, &[]),
        Err(Error::InvalidObjectId)
    );
    assert_eq!(
        host.fetch_request(remote, &["abc".into()]),
        Err(Error::InvalidObjectId)
    );
}

#[test]
fn reports_every_error_kind() {
    let mut host = Host::new();
    assert_eq!(
        host.remote("ssh://example.com/repo").map(|_| ()),
        Err(Error::InvalidUrl)
    );
    let missing = host.remote(&url("missing")).unwrap();
    let reply = send(&host.discovery_request(missing), true);
    assert_eq!(host.accept_discovery(missing, &reply), Err(Error::NotFound));
    let status = |status| Response {
        status,
        content_type: None,
        body: Vec::new(),
    };
    assert_eq!(
        host.accept_discovery(missing, &status(401)),
        Err(Error::AuthRequired(401))
    );
    assert_eq!(
        host.accept_discovery(missing, &status(500)),
        Err(Error::Http(500))
    );
    let html = Response {
        status: 200,
        content_type: Some("text/html".into()),
        body: b"<html>".to_vec(),
    };
    assert_eq!(
        host.accept_discovery(missing, &html),
        Err(Error::NotSmartHttp)
    );

    assert!(matches!(
        host.fetch("sha1", &["1".repeat(40)]),
        Err(Error::Remote(_))
    ));
    assert_eq!(
        host.fetch("sha256", &[rev("sha1", "main")]).map(|_| ()),
        Err(Error::UnsupportedObjectFormat)
    );
    let v0 = host.discover("sha1", false);
    assert_eq!(
        host.fetch_request(v0, &[rev("sha1", "main")]),
        Err(Error::UnsupportedFeature("Git protocol v2".into()))
    );

    let remote = host.discover("sha1", true);
    let main = vec![rev("sha1", "main")];
    let empty = [&b"PACK\0\0\0\x02\0\0\0\0"[..], &[0; 20]].concat();
    assert_eq!(
        host.accept_fetch(remote, &packfile(&empty), &main),
        Err(Error::ChecksumMismatch)
    );
    let huge = [&b"PACK\0\0\0\x02"[..], &200_000u32.to_be_bytes(), &[0; 20]].concat();
    assert_eq!(
        host.accept_fetch(remote, &packfile(&huge), &main),
        Err(Error::LimitExceeded("object count".into()))
    );
    let parent = vec![rev("sha1", "main~1")];
    let request = host.fetch_request(remote, &parent).unwrap();
    assert_eq!(
        host.accept_fetch(remote, &send(&request, true), &main),
        Err(Error::MissingObject(main[0].clone()))
    );
}

#[test]
fn candidates_and_saved_advertisements() {
    let mut host = Host::new();
    assert_eq!(
        host.candidates("v2").unwrap(),
        [
            "v2",
            "refs/v2",
            "refs/tags/v2",
            "refs/heads/v2",
            "refs/remotes/v2",
            "refs/remotes/v2/HEAD"
        ]
    );
    let oid = "a".repeat(40);
    assert_eq!(host.candidates(&oid).unwrap()[0], oid);
    assert_eq!(host.candidates("a..b"), Err(Error::InvalidRefName));

    let remote = host.remote(&url("sha1")).unwrap();
    let v0 = send(&host.discovery_request(remote), false);
    let list = host.parse_advertisement(&v0.body).unwrap();
    assert_eq!(
        get(&list.refs, "refs/heads/main").map(|entry| entry.oid.as_str()),
        Some(rev("sha1", "main").as_str())
    );
    let v2 = send(&host.discovery_request(remote), true);
    assert_eq!(
        host.parse_advertisement(&v2.body),
        Err(Error::Protocol(
            "expected a protocol v0 advertisement".into()
        ))
    );
}
