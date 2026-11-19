use git_remote_refs::{
    Capabilities, Discovery, Error, Limits, Object, ObjectId, Ref, RefList, Remote as Core,
    Request, Resolution, Response, Signature,
};
use js_sys::{Array, Map, Object as JsObject, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
export type ObjectFormat = 'sha1' | 'sha256';
export type ObjectKind = 'commit' | 'tree' | 'blob' | 'tag';

export interface Ref {
    oid: string;
    peeled?: string;
    symrefTarget?: string;
}

export interface RefList {
    objectFormat: ObjectFormat;
    refs: Map<string, Ref>;
    unborn: Map<string, string>;
    prefixes: string[] | null;
}

export type Resolution =
    | { kind: 'ref'; name: string; ref: Ref; others: string[] }
    | { kind: 'object'; oid: string; refs: string[] }
    | { kind: 'not-found' }
    | { kind: 'incomplete'; missing: string[] };

export interface Signature {
    name: string;
    nameBytes?: Uint8Array;
    email: string;
    emailBytes?: Uint8Array;
    time: number;
    offsetMinutes: number;
}

export interface TreeEntry {
    mode: number;
    name: string;
    nameBytes?: Uint8Array;
    oid: string;
}

export type GitObject =
    | {
        kind: 'commit';
        oid: string;
        tree: string;
        parents: string[];
        author: Signature;
        committer: Signature;
        encoding?: string;
        message: string;
        messageBytes?: Uint8Array;
    }
    | {
        kind: 'tag';
        oid: string;
        target: string;
        targetKind: ObjectKind;
        name: string;
        nameBytes?: Uint8Array;
        tagger?: Signature;
        message: string;
        messageBytes?: Uint8Array;
    }
    | { kind: 'tree'; oid: string; entries: TreeEntry[] }
    | { kind: 'blob'; oid: string; data: Uint8Array };

export interface GitRequest {
    method: 'GET' | 'POST';
    url: string;
    headers: Record<string, string>;
    body?: Uint8Array;
}
"#;

fn set(target: &JsObject, key: &str, value: impl Into<JsValue>) -> Result<(), JsValue> {
    Reflect::set(target, &key.into(), &value.into()).map(drop)
}

fn error(error: Error) -> JsValue {
    let value = js_sys::Error::new(&error.to_string());
    let mut result = set(&value, "code", error.code());
    if let Some(status) = error.status() {
        result = result.and_then(|()| set(&value, "status", status));
    }
    match result {
        Ok(()) => value.into(),
        Err(failure) => failure,
    }
}

fn set_text(target: &JsObject, key: &str, bytes: &[u8]) -> Result<(), JsValue> {
    match std::str::from_utf8(bytes) {
        Ok(text) => set(target, key, text),
        Err(_) => {
            set(target, key, String::from_utf8_lossy(bytes).as_ref())?;
            set(target, &format!("{key}Bytes"), Uint8Array::from(bytes))
        }
    }
}

fn strings(values: impl IntoIterator<Item = String>) -> Array {
    values.into_iter().map(JsValue::from).collect()
}

fn request(request: Request) -> Result<JsValue, JsValue> {
    let value = JsObject::new();
    set(&value, "method", request.method.as_str())?;
    set(&value, "url", request.url)?;
    let headers = JsObject::new();
    for (name, header) in request.headers {
        set(&headers, name, header)?;
    }
    set(&value, "headers", headers)?;
    if !request.body.is_empty() {
        set(&value, "body", Uint8Array::from(request.body.as_slice()))?;
    }
    Ok(value.into())
}

fn reference(entry: &Ref) -> Result<JsObject, JsValue> {
    let value = JsObject::new();
    set(&value, "oid", entry.oid.to_hex())?;
    if let Some(peeled) = &entry.peeled {
        set(&value, "peeled", peeled.to_hex())?;
    }
    if let Some(target) = &entry.symref_target {
        set(&value, "symrefTarget", target.as_str())?;
    }
    Ok(value)
}

fn ref_list(list: &RefList) -> Result<JsValue, JsValue> {
    let value = JsObject::new();
    set(&value, "objectFormat", list.format.name())?;
    let refs = Map::new();
    for (name, entry) in &list.refs {
        refs.set(&name.into(), &reference(entry)?.into());
    }
    set(&value, "refs", refs)?;
    let unborn = Map::new();
    for (name, target) in &list.unborn {
        unborn.set(&name.into(), &target.into());
    }
    set(&value, "unborn", unborn)?;
    let prefixes = match &list.prefixes {
        Some(prefixes) => strings(prefixes.iter().cloned()).into(),
        None => JsValue::NULL,
    };
    set(&value, "prefixes", prefixes)?;
    Ok(value.into())
}

fn signature(signature: &Signature) -> Result<JsObject, JsValue> {
    let value = JsObject::new();
    set_text(&value, "name", &signature.name)?;
    set_text(&value, "email", &signature.email)?;
    set(&value, "time", signature.time as f64)?;
    set(&value, "offsetMinutes", signature.offset_minutes)?;
    Ok(value)
}

fn object(oid: &ObjectId, object: &Object) -> Result<JsValue, JsValue> {
    let value = JsObject::new();
    set(&value, "kind", object.kind().name())?;
    set(&value, "oid", oid.to_hex())?;
    match object {
        Object::Commit(commit) => {
            set(&value, "tree", commit.tree.to_hex())?;
            set(
                &value,
                "parents",
                strings(commit.parents.iter().map(ObjectId::to_hex)),
            )?;
            set(&value, "author", signature(&commit.author)?)?;
            set(&value, "committer", signature(&commit.committer)?)?;
            if let Some(encoding) = &commit.encoding {
                set_text(&value, "encoding", encoding)?;
            }
            set_text(&value, "message", &commit.message)?;
        }
        Object::Tag(tag) => {
            set(&value, "target", tag.target.to_hex())?;
            set(&value, "targetKind", tag.target_kind.name())?;
            set_text(&value, "name", &tag.name)?;
            if let Some(tagger) = &tag.tagger {
                set(&value, "tagger", signature(tagger)?)?;
            }
            set_text(&value, "message", &tag.message)?;
        }
        Object::Tree(entries) => {
            let list = Array::new();
            for entry in entries {
                let item = JsObject::new();
                set(&item, "mode", entry.mode)?;
                set_text(&item, "name", &entry.name)?;
                set(&item, "oid", entry.oid.to_hex())?;
                list.push(&item);
            }
            set(&value, "entries", list)?;
        }
        Object::Blob(data) => set(&value, "data", Uint8Array::from(data.as_slice()))?,
    }
    Ok(value.into())
}

fn resolution(resolution: Resolution) -> Result<JsValue, JsValue> {
    let value = JsObject::new();
    match resolution {
        Resolution::Ref {
            name,
            target,
            others,
        } => {
            set(&value, "kind", "ref")?;
            set(&value, "name", name)?;
            set(&value, "ref", reference(&target)?)?;
            set(&value, "others", strings(others))?;
        }
        Resolution::Object { oid, refs } => {
            set(&value, "kind", "object")?;
            set(&value, "oid", oid.to_hex())?;
            set(&value, "refs", strings(refs))?;
        }
        Resolution::NotFound => set(&value, "kind", "not-found")?,
        Resolution::Incomplete { missing } => {
            set(&value, "kind", "incomplete")?;
            set(&value, "missing", strings(missing))?;
        }
    }
    Ok(value.into())
}

fn response<'a>(status: u16, content_type: Option<&'a str>, body: &'a [u8]) -> Response<'a> {
    Response {
        status,
        content_type,
        body,
    }
}

fn object_ids(oids: &[String]) -> Result<Vec<ObjectId>, JsValue> {
    oids.iter().map(|oid| oid.parse().map_err(error)).collect()
}

fn not_ready() -> JsValue {
    error(Error::Protocol("discovery has not completed"))
}

#[wasm_bindgen(js_name = Remote)]
pub struct JsRemote {
    remote: Core,
    discovery: Option<Discovery>,
    list: Option<RefList>,
    prefixes: Vec<String>,
}

#[wasm_bindgen(js_class = Remote)]
impl JsRemote {
    #[wasm_bindgen(constructor)]
    pub fn new(url: &str) -> Result<JsRemote, JsValue> {
        Ok(Self {
            remote: Core::new(url).map_err(error)?,
            discovery: None,
            list: None,
            prefixes: Vec::new(),
        })
    }

    #[wasm_bindgen(js_name = discoveryRequest, unchecked_return_type = "GitRequest")]
    pub fn discovery_request(&self) -> Result<JsValue, JsValue> {
        request(self.remote.discovery_request())
    }

    #[wasm_bindgen(js_name = acceptDiscovery)]
    pub fn accept_discovery(
        &mut self,
        status: u16,
        content_type: Option<String>,
        body: &[u8],
    ) -> Result<(), JsValue> {
        let discovery =
            git_remote_refs::parse_discovery(&response(status, content_type.as_deref(), body))
                .map_err(error)?;
        self.discovery = Some(discovery);
        Ok(())
    }

    #[wasm_bindgen(js_name = listRequest, unchecked_return_type = "GitRequest | undefined")]
    pub fn list_request(&mut self, prefixes: Vec<String>) -> Result<JsValue, JsValue> {
        let borrowed: Vec<&str> = prefixes.iter().map(String::as_str).collect();
        match self.discovery.as_ref().ok_or_else(not_ready)? {
            Discovery::V0(list) => {
                self.list = Some(list.clone().restrict(&borrowed).map_err(error)?);
                Ok(JsValue::UNDEFINED)
            }
            Discovery::V2(capabilities) => {
                let pending = self
                    .remote
                    .ls_refs_request(capabilities, &borrowed)
                    .map_err(error)?;
                self.prefixes = prefixes;
                request(pending)
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
        let list = git_remote_refs::parse_ls_refs(
            capabilities,
            &response(status, content_type.as_deref(), body),
            &prefixes,
        )
        .map_err(error)?;
        self.list = Some(list);
        Ok(())
    }

    #[wasm_bindgen(unchecked_return_type = "RefList")]
    pub fn refs(&self) -> Result<JsValue, JsValue> {
        ref_list(self.list.as_ref().ok_or_else(not_ready)?)
    }

    #[wasm_bindgen(unchecked_return_type = "Resolution")]
    pub fn resolve(&self, name: &str) -> Result<JsValue, JsValue> {
        let list = self.list.as_ref().ok_or_else(not_ready)?;
        resolution(git_remote_refs::resolve(list, name).map_err(error)?)
    }

    #[wasm_bindgen(js_name = fetchRequest, unchecked_return_type = "GitRequest")]
    pub fn fetch_request(&self, oids: Vec<String>) -> Result<JsValue, JsValue> {
        let wants = object_ids(&oids)?;
        request(
            self.remote
                .fetch_request(self.capabilities()?, &wants)
                .map_err(error)?,
        )
    }

    #[wasm_bindgen(js_name = acceptFetch, unchecked_return_type = "Map<string, GitObject>")]
    pub fn accept_fetch(
        &self,
        status: u16,
        content_type: Option<String>,
        body: &[u8],
        oids: Vec<String>,
    ) -> Result<JsValue, JsValue> {
        let wants = object_ids(&oids)?;
        let objects = git_remote_refs::parse_fetch(
            self.capabilities()?,
            &response(status, content_type.as_deref(), body),
            &wants,
            &Limits::default(),
        )
        .map_err(error)?;
        let map = Map::new();
        for (oid, parsed) in &objects {
            map.set(&oid.to_hex().into(), &object(oid, parsed)?);
        }
        Ok(map.into())
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
        git_remote_refs::check_ref_name(name).map_err(error)?;
    }
    Ok(git_remote_refs::candidates(name))
}

#[wasm_bindgen(js_name = parseAdvertisement, unchecked_return_type = "RefList")]
pub fn parse_advertisement(bytes: &[u8]) -> Result<JsValue, JsValue> {
    ref_list(&git_remote_refs::parse_advertisement(bytes).map_err(error)?)
}
