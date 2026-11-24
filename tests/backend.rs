use git_remote_refs::{
    Capabilities, Discovery, Error, Fetched, Kind, Limits, Object, ObjectFormat, ObjectId, RefList,
    Remote, Request, Resolution, Response, candidates, parse_discovery, parse_fetch, parse_ls_refs,
    resolve,
};
use std::collections::BTreeMap;
use support::Reply;

mod support;

impl Reply {
    fn response(&self) -> Response<'_> {
        Response {
            status: self.status,
            content_type: self.content_type.as_deref(),
            body: &self.body,
        }
    }
}

fn exchange(request: &Request, v2: bool) -> Reply {
    support::exchange(
        request.method.as_str(),
        &request.url,
        request.headers.iter().copied(),
        &request.body,
        v2,
    )
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

fn fetched(name: &str, wants: &[ObjectId]) -> Result<Fetched, Error> {
    let capabilities = capabilities(name);
    let request = remote(name).fetch_request(&capabilities, wants)?;
    parse_fetch(
        &capabilities,
        &exchange(&request, true).response(),
        wants,
        &Limits::default(),
    )
}

fn fetch(name: &str, wants: &[ObjectId]) -> Result<BTreeMap<ObjectId, Object>, Error> {
    fetched(name, wants).map(|fetched| fetched.objects)
}

fn rev(name: &str, revision: &str) -> ObjectId {
    support::rev(name, revision).parse().unwrap()
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
fn quickstart_resolves_with_candidate_prefixes() {
    let remote = remote("sha1");
    let capabilities = capabilities("sha1");
    let list_candidates = |name: &str| {
        let names = candidates(name);
        let prefixes: Vec<_> = names.iter().map(String::as_str).collect();
        let request = remote.ls_refs_request(&capabilities, &prefixes).unwrap();
        let body = std::str::from_utf8(&request.body).unwrap();
        let sent: Vec<_> = body
            .lines()
            .filter_map(|line| line.get(4..)?.strip_prefix("ref-prefix "))
            .collect();
        assert_eq!(sent.len(), 6);
        assert_eq!(sent, prefixes);

        let reply = exchange(&request, true);
        parse_ls_refs(&capabilities, &reply.response(), &prefixes).unwrap()
    };

    let refs = list_candidates("v2");
    assert_eq!(
        refs.get("refs/heads/v2").unwrap().oid,
        rev("sha1", "refs/heads/v2")
    );
    let Resolution::Ref {
        name,
        target,
        others,
    } = resolve(&refs, "v2").unwrap()
    else {
        panic!("expected tag and branch collision to resolve");
    };
    assert_eq!(name, "refs/tags/v2");
    assert_eq!(target.oid, rev("sha1", "refs/tags/v2"));
    assert_eq!(others, ["refs/heads/v2"]);
    assert_eq!(
        resolve(&list_candidates("missing"), "missing"),
        Ok(Resolution::NotFound)
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
    let blobs = fetched("sha1", &[empty, file]).unwrap();
    assert_eq!(
        blobs.wanted,
        [
            (empty, Object::Blob(Vec::new())),
            (file, Object::Blob(b"one\n".to_vec())),
        ]
    );
    assert_eq!(blobs.objects.len(), 2);
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

#[test]
fn packs_without_the_wanted_object_are_rejected() {
    let capabilities = capabilities("sha1");
    let main = rev("sha1", "main");
    let parent = rev("sha1", "main~1");
    let request = remote("sha1")
        .fetch_request(&capabilities, &[parent])
        .unwrap();
    assert_eq!(
        parse_fetch(
            &capabilities,
            &exchange(&request, true).response(),
            &[main],
            &Limits::default(),
        ),
        Err(Error::MissingObject(main))
    );
}
