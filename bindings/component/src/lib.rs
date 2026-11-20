use exports::kjanat::git_remote_refs::git::{
    Blob, Commit, Error, Fetched, GitObject, GitRef, Guest, GuestRemote, Method, ObjectFormat,
    ObjectKind, ObjectMatch, RefList, RefMatch, Request, Resolution, Response, Signature, Tag,
    Tree, TreeEntry,
};
use git_remote_refs as git;
use git_remote_refs::{Capabilities, Discovery, Limits, ObjectId};
use std::cell::RefCell;

wit_bindgen::generate!({ world: "git-remote-refs" });

struct Component;

export!(Component);

impl From<git::Error> for Error {
    fn from(error: git::Error) -> Self {
        match error {
            git::Error::InvalidUrl => Self::InvalidUrl,
            git::Error::InvalidRefName => Self::InvalidRefName,
            git::Error::InvalidPrefix => Self::InvalidPrefix,
            git::Error::InvalidObjectId => Self::InvalidObjectId,
            git::Error::AuthRequired(status) => Self::AuthRequired(status),
            git::Error::NotFound => Self::NotFound,
            git::Error::Http(status) => Self::Http(status),
            git::Error::NotSmartHttp => Self::NotSmartHttp,
            git::Error::Protocol(message) => Self::Protocol(message.into()),
            git::Error::Remote(message) => Self::Remote(message),
            git::Error::UnsupportedObjectFormat => Self::UnsupportedObjectFormat,
            git::Error::UnsupportedFeature(feature) => Self::UnsupportedFeature(feature.into()),
            git::Error::ChecksumMismatch => Self::ChecksumMismatch,
            git::Error::LimitExceeded(limit) => Self::LimitExceeded(limit.into()),
            git::Error::MissingObject(oid) => Self::MissingObject(oid.to_hex()),
        }
    }
}

impl From<git::Request> for Request {
    fn from(request: git::Request) -> Self {
        Self {
            method: match request.method {
                git::Method::Get => Method::Get,
                git::Method::Post => Method::Post,
            },
            url: request.url,
            headers: request
                .headers
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
            body: request.body,
        }
    }
}

impl From<&git::Ref> for GitRef {
    fn from(entry: &git::Ref) -> Self {
        Self {
            oid: entry.oid.to_hex(),
            peeled: entry.peeled.as_ref().map(ObjectId::to_hex),
            symref_target: entry.symref_target.clone(),
        }
    }
}

impl From<&git::RefList> for RefList {
    fn from(list: &git::RefList) -> Self {
        Self {
            object_format: match list.format {
                git::ObjectFormat::Sha1 => ObjectFormat::Sha1,
                git::ObjectFormat::Sha256 => ObjectFormat::Sha256,
            },
            refs: list
                .refs
                .iter()
                .map(|(name, entry)| (name.clone(), entry.into()))
                .collect(),
            unborn: list.unborn.clone().into_iter().collect(),
            prefixes: list.prefixes.clone(),
        }
    }
}

impl From<git::Resolution> for Resolution {
    fn from(resolution: git::Resolution) -> Self {
        match resolution {
            git::Resolution::Ref {
                name,
                target,
                others,
            } => Self::FoundRef(RefMatch {
                name,
                target: (&target).into(),
                others,
            }),
            git::Resolution::Object { oid, refs } => Self::FoundObject(ObjectMatch {
                oid: oid.to_hex(),
                refs,
            }),
            git::Resolution::NotFound => Self::NotFound,
            git::Resolution::Incomplete { missing } => Self::Incomplete(missing),
        }
    }
}

impl From<git::Kind> for ObjectKind {
    fn from(kind: git::Kind) -> Self {
        match kind {
            git::Kind::Commit => Self::Commit,
            git::Kind::Tree => Self::Tree,
            git::Kind::Blob => Self::Blob,
            git::Kind::Tag => Self::Tag,
        }
    }
}

fn text(bytes: &[u8]) -> (String, Option<Vec<u8>>) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.into(), None),
        Err(_) => (
            String::from_utf8_lossy(bytes).into_owned(),
            Some(bytes.to_vec()),
        ),
    }
}

fn hex(oids: &[ObjectId]) -> Vec<String> {
    oids.iter().map(ObjectId::to_hex).collect()
}

impl From<&git::Signature> for Signature {
    fn from(signature: &git::Signature) -> Self {
        let (name, name_bytes) = text(&signature.name);
        let (email, email_bytes) = text(&signature.email);
        Self {
            name,
            name_bytes,
            email,
            email_bytes,
            time: signature.time,
            offset_minutes: signature.offset_minutes,
        }
    }
}

fn object(oid: &ObjectId, object: &git::Object) -> GitObject {
    let oid = oid.to_hex();
    match object {
        git::Object::Commit(commit) => {
            let (message, message_bytes) = text(&commit.message);
            GitObject::Commit(Commit {
                oid,
                tree: commit.tree.to_hex(),
                parents: hex(&commit.parents),
                author: (&commit.author).into(),
                committer: (&commit.committer).into(),
                encoding: commit.encoding.as_deref().map(|encoding| text(encoding).0),
                message,
                message_bytes,
            })
        }
        git::Object::Tag(tag) => {
            let (name, name_bytes) = text(&tag.name);
            let (message, message_bytes) = text(&tag.message);
            GitObject::Tag(Tag {
                oid,
                target: tag.target.to_hex(),
                target_kind: tag.target_kind.into(),
                name,
                name_bytes,
                tagger: tag.tagger.as_ref().map(Into::into),
                message,
                message_bytes,
            })
        }
        git::Object::Tree(entries) => GitObject::Tree(Tree {
            oid,
            entries: entries
                .iter()
                .map(|entry| {
                    let (name, name_bytes) = text(&entry.name);
                    TreeEntry {
                        mode: entry.mode,
                        name,
                        name_bytes,
                        oid: entry.oid.to_hex(),
                    }
                })
                .collect(),
        }),
        git::Object::Blob(data) => GitObject::Blob(Blob {
            oid,
            data: data.clone(),
        }),
    }
}

fn response(response: &Response) -> git::Response<'_> {
    git::Response {
        status: response.status,
        content_type: response.content_type.as_deref(),
        body: &response.body,
    }
}

fn object_ids(oids: &[String]) -> Result<Vec<ObjectId>, Error> {
    if oids.is_empty() {
        return Err(git::Error::InvalidObjectId.into());
    }
    oids.iter()
        .map(|oid| oid.parse().map_err(Error::from))
        .collect()
}

fn not_ready() -> Error {
    git::Error::Protocol("discovery has not completed").into()
}

#[derive(Default)]
struct State {
    discovery: Option<Discovery>,
    list: Option<git::RefList>,
    prefixes: Vec<String>,
}

impl State {
    fn capabilities(&self) -> Result<&Capabilities, Error> {
        match self.discovery.as_ref().ok_or_else(not_ready)? {
            Discovery::V2(capabilities) => Ok(capabilities),
            Discovery::V0(_) => Err(git::Error::UnsupportedFeature("Git protocol v2").into()),
        }
    }

    fn list(&self) -> Result<&git::RefList, Error> {
        self.list.as_ref().ok_or_else(not_ready)
    }
}

struct Remote {
    remote: git::Remote,
    state: RefCell<State>,
}

impl GuestRemote for Remote {
    fn new(url: String) -> Result<Self, Error> {
        Ok(Self {
            remote: git::Remote::new(&url)?,
            state: RefCell::default(),
        })
    }

    fn discovery_request(&self) -> Request {
        self.remote.discovery_request().into()
    }

    fn accept_discovery(&self, reply: Response) -> Result<(), Error> {
        self.state.borrow_mut().discovery = Some(git::parse_discovery(&response(&reply))?);
        Ok(())
    }

    fn list_request(&self, prefixes: Vec<String>) -> Result<Option<Request>, Error> {
        let mut state = self.state.borrow_mut();
        let borrowed: Vec<&str> = prefixes.iter().map(String::as_str).collect();
        match state.discovery.as_ref().ok_or_else(not_ready)? {
            Discovery::V0(list) => {
                let list = list.clone().restrict(&borrowed)?;
                state.list = Some(list);
                Ok(None)
            }
            Discovery::V2(capabilities) => {
                let pending = self.remote.ls_refs_request(capabilities, &borrowed)?;
                state.prefixes = prefixes;
                Ok(Some(pending.into()))
            }
        }
    }

    fn accept_list(&self, reply: Response) -> Result<(), Error> {
        let mut state = self.state.borrow_mut();
        let prefixes: Vec<&str> = state.prefixes.iter().map(String::as_str).collect();
        let list = git::parse_ls_refs(state.capabilities()?, &response(&reply), &prefixes)?;
        state.list = Some(list);
        Ok(())
    }

    fn refs(&self) -> Result<RefList, Error> {
        Ok(self.state.borrow().list()?.into())
    }

    fn resolve(&self, name: String) -> Result<Resolution, Error> {
        Ok(git::resolve(self.state.borrow().list()?, &name)?.into())
    }

    fn fetch_request(&self, oids: Vec<String>) -> Result<Request, Error> {
        let wants = object_ids(&oids)?;
        let state = self.state.borrow();
        Ok(self
            .remote
            .fetch_request(state.capabilities()?, &wants)?
            .into())
    }

    fn accept_fetch(&self, reply: Response, oids: Vec<String>) -> Result<Fetched, Error> {
        let wants = object_ids(&oids)?;
        let fetched = git::parse_fetch(
            self.state.borrow().capabilities()?,
            &response(&reply),
            &wants,
            &Limits::default(),
        )?;
        Ok(Fetched {
            wanted: fetched
                .wanted
                .iter()
                .map(|(oid, entry)| object(oid, entry))
                .collect(),
            objects: fetched
                .objects
                .iter()
                .map(|(oid, entry)| (oid.to_hex(), object(oid, entry)))
                .collect(),
        })
    }
}

impl Guest for Component {
    type Remote = Remote;

    fn candidates(name: String) -> Result<Vec<String>, Error> {
        if ObjectId::from_hex(name.as_bytes()).is_none() {
            git::check_ref_name(&name)?;
        }
        Ok(git::candidates(&name))
    }

    fn parse_advertisement(body: Vec<u8>) -> Result<RefList, Error> {
        Ok((&git::parse_advertisement(&body)?).into())
    }
}
