#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod discovery;
mod error;
mod fetch;
mod http;
mod object;
mod oid;
mod pack;
mod pkt;
mod refs;
mod resolve;

pub use discovery::{Capabilities, Discovery, parse_advertisement, parse_discovery};
pub use error::Error;
pub use fetch::{Fetched, parse_fetch};
pub use http::{Method, Remote, Request, Response};
pub use object::{Commit, Kind, Object, Signature, Tag, TreeEntry, object_id};
pub use oid::{ObjectFormat, ObjectId};
pub use pack::Limits;
pub use refs::{Ref, RefList, check_ref_name, parse_ls_refs};
pub use resolve::{Resolution, candidates, resolve};
