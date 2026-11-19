use git_remote_refs as git;
use git_remote_refs::{Capabilities, Discovery, Error, Limits, ObjectId};
use js_sys::Reflect;
use serde::Serialize;
use serde_bytes::ByteBuf;
use std::collections::BTreeMap;
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;

#[derive(Serialize, Tsify)]
#[serde(rename_all = "lowercase")]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "lowercase")]
pub enum ObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct Ref {
    oid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    peeled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    symref_target: Option<String>,
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
#[tsify(missing_as_null)]
pub struct RefList {
    object_format: ObjectFormat,
    refs: BTreeMap<String, Ref>,
    unborn: BTreeMap<String, String>,
    prefixes: Option<Vec<String>>,
}

#[derive(Serialize, Tsify)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Resolution {
    Ref {
        name: String,
        #[serde(rename = "ref")]
        target: Ref,
        others: Vec<String>,
    },
    Object {
        oid: String,
        refs: Vec<String>,
    },
    NotFound,
    Incomplete {
        missing: Vec<String>,
    },
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[tsify(optional, type = "Uint8Array")]
    name_bytes: Option<ByteBuf>,
    email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[tsify(optional, type = "Uint8Array")]
    email_bytes: Option<ByteBuf>,
    time: i64,
    offset_minutes: i32,
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    mode: u32,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[tsify(optional, type = "Uint8Array")]
    name_bytes: Option<ByteBuf>,
    oid: String,
}

#[derive(Serialize, Tsify)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum GitObject {
    Commit {
        oid: String,
        tree: String,
        parents: Vec<String>,
        author: Signature,
        committer: Signature,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        encoding: Option<String>,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional, type = "Uint8Array")]
        message_bytes: Option<ByteBuf>,
    },
    Tag {
        oid: String,
        target: String,
        target_kind: ObjectKind,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional, type = "Uint8Array")]
        name_bytes: Option<ByteBuf>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        tagger: Option<Signature>,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional, type = "Uint8Array")]
        message_bytes: Option<ByteBuf>,
    },
    Tree {
        oid: String,
        entries: Vec<TreeEntry>,
    },
    Blob {
        oid: String,
        #[tsify(type = "Uint8Array")]
        data: ByteBuf,
    },
}

#[derive(Serialize, Tsify)]
pub struct Fetched {
    #[tsify(type = "[GitObject, ...GitObject[]]")]
    wanted: Vec<GitObject>,
    objects: BTreeMap<String, GitObject>,
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
}

#[derive(Serialize, Tsify)]
pub struct GitRequest {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[tsify(optional, type = "Uint8Array")]
    body: Option<ByteBuf>,
}

fn error(error: Error) -> JsValue {
    let value = js_sys::Error::new(&error.to_string());
    let mut result = Reflect::set(&value, &"code".into(), &error.code().into());
    if let Some(status) = error.status() {
        result = result.and_then(|_| Reflect::set(&value, &"status".into(), &status.into()));
    }
    match result {
        Ok(_) => value.into(),
        Err(failure) => failure,
    }
}

fn ts<T: Tsify + Serialize>(value: &T) -> Result<Ts<T>, JsValue> {
    value
        .into_ts()
        .map_err(|failure| JsError::from(failure).into())
}

fn text(bytes: &[u8]) -> (String, Option<ByteBuf>) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.into(), None),
        Err(_) => (
            String::from_utf8_lossy(bytes).into_owned(),
            Some(ByteBuf::from(bytes)),
        ),
    }
}

fn hex(oids: &[ObjectId]) -> Vec<String> {
    oids.iter().map(ObjectId::to_hex).collect()
}

impl From<git::Request> for GitRequest {
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
            body: (!request.body.is_empty()).then(|| ByteBuf::from(request.body)),
        }
    }
}

impl From<&git::Ref> for Ref {
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
            unborn: list.unborn.clone(),
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
            } => Self::Ref {
                name,
                target: (&target).into(),
                others,
            },
            git::Resolution::Object { oid, refs } => Self::Object {
                oid: oid.to_hex(),
                refs,
            },
            git::Resolution::NotFound => Self::NotFound,
            git::Resolution::Incomplete { missing } => Self::Incomplete { missing },
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

impl GitObject {
    fn new(oid: &ObjectId, object: &git::Object) -> Self {
        let oid = oid.to_hex();
        match object {
            git::Object::Commit(commit) => {
                let (message, message_bytes) = text(&commit.message);
                Self::Commit {
                    oid,
                    tree: commit.tree.to_hex(),
                    parents: hex(&commit.parents),
                    author: (&commit.author).into(),
                    committer: (&commit.committer).into(),
                    encoding: commit.encoding.as_deref().map(|encoding| text(encoding).0),
                    message,
                    message_bytes,
                }
            }
            git::Object::Tag(tag) => {
                let (name, name_bytes) = text(&tag.name);
                let (message, message_bytes) = text(&tag.message);
                Self::Tag {
                    oid,
                    target: tag.target.to_hex(),
                    target_kind: tag.target_kind.into(),
                    name,
                    name_bytes,
                    tagger: tag.tagger.as_ref().map(Into::into),
                    message,
                    message_bytes,
                }
            }
            git::Object::Tree(entries) => Self::Tree {
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
            },
            git::Object::Blob(data) => Self::Blob {
                oid,
                data: ByteBuf::from(data.as_slice()),
            },
        }
    }
}

fn response<'a>(status: u16, content_type: Option<&'a str>, body: &'a [u8]) -> git::Response<'a> {
    git::Response {
        status,
        content_type,
        body,
    }
}

fn object_ids(oids: &[String]) -> Result<Vec<ObjectId>, JsValue> {
    if oids.is_empty() {
        return Err(error(Error::InvalidObjectId));
    }
    oids.iter().map(|oid| oid.parse().map_err(error)).collect()
}

fn not_ready() -> JsValue {
    error(Error::Protocol("discovery has not completed"))
}

#[wasm_bindgen(js_name = Remote)]
pub struct JsRemote {
    remote: git::Remote,
    discovery: Option<Discovery>,
    list: Option<git::RefList>,
    prefixes: Vec<String>,
}

#[wasm_bindgen(js_class = Remote)]
impl JsRemote {
    #[wasm_bindgen(constructor)]
    pub fn new(url: &str) -> Result<JsRemote, JsValue> {
        Ok(Self {
            remote: git::Remote::new(url).map_err(error)?,
            discovery: None,
            list: None,
            prefixes: Vec::new(),
        })
    }

    #[wasm_bindgen(js_name = discoveryRequest)]
    pub fn discovery_request(&self) -> Result<Ts<GitRequest>, JsValue> {
        ts(&self.remote.discovery_request().into())
    }

    #[wasm_bindgen(js_name = acceptDiscovery)]
    pub fn accept_discovery(
        &mut self,
        status: u16,
        content_type: Option<String>,
        body: &[u8],
    ) -> Result<(), JsValue> {
        let discovery = git::parse_discovery(&response(status, content_type.as_deref(), body))
            .map_err(error)?;
        self.discovery = Some(discovery);
        Ok(())
    }

    #[wasm_bindgen(js_name = listRequest)]
    pub fn list_request(
        &mut self,
        prefixes: Vec<String>,
    ) -> Result<Option<Ts<GitRequest>>, JsValue> {
        let borrowed: Vec<&str> = prefixes.iter().map(String::as_str).collect();
        match self.discovery.as_ref().ok_or_else(not_ready)? {
            Discovery::V0(list) => {
                self.list = Some(list.clone().restrict(&borrowed).map_err(error)?);
                Ok(None)
            }
            Discovery::V2(capabilities) => {
                let pending = self
                    .remote
                    .ls_refs_request(capabilities, &borrowed)
                    .map_err(error)?;
                self.prefixes = prefixes;
                ts(&GitRequest::from(pending)).map(Some)
            }
        }
    }

    #[wasm_bindgen(js_name = acceptList)]
    pub fn accept_list(
        &mut self,
        status: u16,
        content_type: Option<String>,
        body: &[u8],
    ) -> Result<(), JsValue> {
        let capabilities = self.capabilities()?;
        let prefixes: Vec<&str> = self.prefixes.iter().map(String::as_str).collect();
        let list = git::parse_ls_refs(
            capabilities,
            &response(status, content_type.as_deref(), body),
            &prefixes,
        )
        .map_err(error)?;
        self.list = Some(list);
        Ok(())
    }

    pub fn refs(&self) -> Result<Ts<RefList>, JsValue> {
        ts(&RefList::from(self.list.as_ref().ok_or_else(not_ready)?))
    }

    pub fn resolve(&self, name: &str) -> Result<Ts<Resolution>, JsValue> {
        let list = self.list.as_ref().ok_or_else(not_ready)?;
        ts(&Resolution::from(git::resolve(list, name).map_err(error)?))
    }

    #[wasm_bindgen(js_name = fetchRequest)]
    pub fn fetch_request(&self, oids: Vec<String>) -> Result<Ts<GitRequest>, JsValue> {
        let wants = object_ids(&oids)?;
        let request = self
            .remote
            .fetch_request(self.capabilities()?, &wants)
            .map_err(error)?;
        ts(&GitRequest::from(request))
    }

    #[wasm_bindgen(js_name = acceptFetch)]
    pub fn accept_fetch(
        &self,
        status: u16,
        content_type: Option<String>,
        body: &[u8],
        oids: Vec<String>,
    ) -> Result<Ts<Fetched>, JsValue> {
        let wants = object_ids(&oids)?;
        let fetched = git::parse_fetch(
            self.capabilities()?,
            &response(status, content_type.as_deref(), body),
            &wants,
            &Limits::default(),
        )
        .map_err(error)?;
        ts(&Fetched {
            wanted: fetched
                .wanted
                .iter()
                .map(|(oid, object)| GitObject::new(oid, object))
                .collect(),
            objects: fetched
                .objects
                .iter()
                .map(|(oid, object)| (oid.to_hex(), GitObject::new(oid, object)))
                .collect(),
        })
    }

    fn capabilities(&self) -> Result<&Capabilities, JsValue> {
        match self.discovery.as_ref().ok_or_else(not_ready)? {
            Discovery::V2(capabilities) => Ok(capabilities),
            Discovery::V0(_) => Err(error(Error::UnsupportedFeature("Git protocol v2"))),
        }
    }
}

#[wasm_bindgen]
pub fn candidates(name: &str) -> Result<Vec<String>, JsValue> {
    if ObjectId::from_hex(name.as_bytes()).is_none() {
        git::check_ref_name(name).map_err(error)?;
    }
    Ok(git::candidates(name))
}

#[wasm_bindgen(js_name = parseAdvertisement)]
pub fn parse_advertisement(bytes: &[u8]) -> Result<Ts<RefList>, JsValue> {
    ts(&RefList::from(
        &git::parse_advertisement(bytes).map_err(error)?,
    ))
}
