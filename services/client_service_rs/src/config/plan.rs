use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::config::document::{ConfigDocument, ConfigSpec};

/// The kind of change planned for a single resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanAction {
    Create,
    Update,
    Delete,
    NoOp,
}

impl PlanAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::NoOp => "no-op",
        }
    }
}

/// A single redacted resource-level change entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanEntry {
    pub kind: String,
    pub id: String,
    pub action: PlanAction,
}

/// A redacted create/update/delete/no-op plan comparing two documents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Plan {
    pub entries: Vec<PlanEntry>,
}

impl Plan {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut summary: BTreeMap<&str, usize> = BTreeMap::new();
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|entry| {
                *summary.entry(entry.action.as_str()).or_insert(0) += 1;
                json!({
                    "kind": entry.kind,
                    "id": entry.id,
                    "action": entry.action.as_str(),
                })
            })
            .collect();
        json!({
            "changes": entries,
            "summary": {
                "create": summary.get("create").copied().unwrap_or(0),
                "update": summary.get("update").copied().unwrap_or(0),
                "delete": summary.get("delete").copied().unwrap_or(0),
                "no-op": summary.get("no-op").copied().unwrap_or(0),
            },
        })
    }
}

/// Compute a redacted plan diffing `next` against `current`. Because the
/// document contains only `secretRef` descriptors (never values), plan output
/// is inherently free of secret values.
#[must_use]
pub fn plan(current: &ConfigDocument, next: &ConfigDocument) -> Plan {
    let mut entries = Vec::new();
    diff_collection(
        &mut entries,
        "secret",
        &collect(&current.spec, |spec| {
            spec.secrets
                .iter()
                .map(|item| (item.name.as_str().to_owned(), json_of(item)))
                .collect()
        }),
        &collect(&next.spec, |spec| {
            spec.secrets
                .iter()
                .map(|item| (item.name.as_str().to_owned(), json_of(item)))
                .collect()
        }),
    );
    diff_collection(
        &mut entries,
        "kernelConfig",
        &map_of(&current.spec.kernel_configs, |item| {
            (item.harness.as_str().to_owned(), json_of(item))
        }),
        &map_of(&next.spec.kernel_configs, |item| {
            (item.harness.as_str().to_owned(), json_of(item))
        }),
    );
    diff_collection(
        &mut entries,
        "connection",
        &map_of(&current.spec.connections, |item| {
            (item.id.clone(), json_of(item))
        }),
        &map_of(&next.spec.connections, |item| {
            (item.id.clone(), json_of(item))
        }),
    );
    diff_collection(
        &mut entries,
        "skill",
        &map_of(&current.spec.skills, |item| {
            (item.id.clone(), json_of(item))
        }),
        &map_of(&next.spec.skills, |item| (item.id.clone(), json_of(item))),
    );
    diff_collection(
        &mut entries,
        "agent",
        &map_of(&current.spec.agents, |item| {
            (item.id.clone(), json_of(item))
        }),
        &map_of(&next.spec.agents, |item| (item.id.clone(), json_of(item))),
    );
    diff_collection(
        &mut entries,
        "gateway",
        &map_of(&current.spec.gateways, |item| {
            (item.id.clone(), json_of(item))
        }),
        &map_of(&next.spec.gateways, |item| (item.id.clone(), json_of(item))),
    );
    Plan { entries }
}

fn collect(
    spec: &ConfigSpec,
    map: impl Fn(&ConfigSpec) -> BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    map(spec)
}

fn map_of<T>(items: &[T], to_entry: impl Fn(&T) -> (String, Value)) -> BTreeMap<String, Value> {
    items.iter().map(to_entry).collect()
}

fn json_of<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn diff_collection(
    entries: &mut Vec<PlanEntry>,
    kind: &str,
    current: &BTreeMap<String, Value>,
    next: &BTreeMap<String, Value>,
) {
    let mut ids: Vec<String> = current.keys().chain(next.keys()).cloned().collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        let action = match (current.get(&id), next.get(&id)) {
            (None, Some(_)) => PlanAction::Create,
            (Some(_), None) => PlanAction::Delete,
            (Some(before), Some(after)) if before == after => PlanAction::NoOp,
            (Some(_), Some(_)) => PlanAction::Update,
            (None, None) => continue,
        };
        entries.push(PlanEntry {
            kind: kind.to_owned(),
            id,
            action,
        });
    }
}
