use crate::{Error, ObjectId, Ref, RefList, check_ref_name};
use alloc::{format, string::String, vec, vec::Vec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Ref {
        name: String,
        target: Ref,
        others: Vec<String>,
    },
    Object {
        oid: ObjectId,
        refs: Vec<String>,
    },
    NotFound,
    Incomplete {
        missing: Vec<String>,
    },
}

pub fn candidates(name: &str) -> Vec<String> {
    vec![
        name.into(),
        format!("refs/{name}"),
        format!("refs/tags/{name}"),
        format!("refs/heads/{name}"),
        format!("refs/remotes/{name}"),
        format!("refs/remotes/{name}/HEAD"),
    ]
}

pub fn resolve(list: &RefList, name: &str) -> Result<Resolution, Error> {
    let oid = ObjectId::from_hex(name.as_bytes()).filter(|oid| oid.format() == list.format);
    if oid.is_none() {
        check_ref_name(name)?;
    }
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for candidate in candidates(name) {
        if list.refs.contains_key(&candidate) {
            found.push(candidate);
        } else if !list.covers(&candidate) {
            missing.push(candidate);
        }
    }
    if !missing.is_empty() {
        return Ok(Resolution::Incomplete { missing });
    }
    if let Some(oid) = oid {
        return Ok(Resolution::Object { oid, refs: found });
    }
    let mut found = found.into_iter();
    Ok(match found.next() {
        Some(name) => Resolution::Ref {
            target: list.refs[&name].clone(),
            name,
            others: found.collect(),
        },
        None => Resolution::NotFound,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ObjectFormat;
    use alloc::collections::BTreeMap;

    fn list(names: &[&str]) -> RefList {
        let oid: ObjectId = "1111111111111111111111111111111111111111".parse().unwrap();
        RefList {
            format: ObjectFormat::Sha1,
            refs: names
                .iter()
                .map(|name| {
                    (
                        String::from(*name),
                        Ref {
                            oid,
                            peeled: None,
                            symref_target: None,
                        },
                    )
                })
                .collect(),
            unborn: BTreeMap::new(),
            prefixes: None,
        }
    }

    #[test]
    fn follows_git_order_and_reports_ambiguity() {
        let refs = list(&[
            "refs/heads/v2",
            "refs/tags/v2",
            "refs/heads/heads/x",
            "refs/heads/x",
        ]);
        let Resolution::Ref { name, others, .. } = resolve(&refs, "v2").unwrap() else {
            panic!("expected ref");
        };
        assert_eq!(
            (name.as_str(), others),
            ("refs/tags/v2", vec![String::from("refs/heads/v2")])
        );
        let Resolution::Ref { name, others, .. } = resolve(&refs, "heads/x").unwrap() else {
            panic!("expected ref");
        };
        assert_eq!(
            (name.as_str(), others),
            ("refs/heads/x", vec![String::from("refs/heads/heads/x")])
        );
        assert_eq!(resolve(&refs, "nope"), Ok(Resolution::NotFound));
        assert_eq!(resolve(&refs, "a..b"), Err(Error::InvalidRefName));
    }

    #[test]
    fn full_object_ids_win_over_refs() {
        let hex = "2222222222222222222222222222222222222222";
        let refs = list(&[&format!("refs/tags/{hex}")]);
        assert_eq!(
            resolve(&refs, hex),
            Ok(Resolution::Object {
                oid: hex.parse().unwrap(),
                refs: vec![format!("refs/tags/{hex}")],
            })
        );
        let sha256 = "a".repeat(64);
        assert_eq!(resolve(&refs, &sha256), Ok(Resolution::NotFound));
    }

    #[test]
    fn partial_listing_reports_unlisted_candidates() {
        let refs = list(&["refs/heads/v2"]).restrict(&["refs/heads/"]).unwrap();
        let Resolution::Incomplete { missing } = resolve(&refs, "v2").unwrap() else {
            panic!("expected incomplete");
        };
        assert_eq!(missing.len(), 5);
        assert!(missing.contains(&String::from("refs/tags/v2")));
        let covered = list(&["refs/heads/v2"])
            .restrict(
                &candidates("v2")
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        assert!(matches!(
            resolve(&covered, "v2"),
            Ok(Resolution::Ref { .. })
        ));
    }
}
