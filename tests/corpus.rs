//! Decode recorded `codex app-server` output with the generated protocol
//! types. `tests/corpus/` holds the server's side of real sessions (paths,
//! host names and ids scrubbed); a schema or generator change that stops
//! them decoding fails here.

use codexwire::protocol::{
    CODEX_VERSION, ErrorNotification, InitializeResponse, ItemCompletedNotification,
    ItemStartedNotification, Notification, ThreadItem, ThreadStartResponse,
    ThreadStartedNotification, TurnCompletedNotification, TurnStartResponse,
    TurnStartedNotification,
};
use codexwire::rpc::{Incoming, parse_line};
use serde::de::DeserializeOwned;
use serde_json::Value;

const TURN: &str = include_str!("corpus/codex-0.158.0-turn-unauthenticated.jsonl");

fn decode<T: DeserializeOwned>(what: &str, v: Value) -> T {
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{what}: {e}"))
}

fn known_item(what: &str, item: &ThreadItem) {
    assert!(
        !matches!(item, ThreadItem::Unknown(_)),
        "{what}: the item fell back to Unknown: {item:?}"
    );
}

/// Decode `params` as `T` if `method` is `T`'s; whether it was.
fn try_as<T: Notification>(method: &str, params: &Value, check: impl Fn(&T)) -> bool {
    if method != T::METHOD {
        return false;
    }
    check(&decode::<T>(method, params.clone()));
    true
}

#[test]
fn the_recorded_turn_decodes() {
    let mut decoded = 0;
    let mut other = Vec::new();
    for (n, line) in TURN.lines().enumerate() {
        let what = format!("line {}", n + 1);
        match parse_line(line).unwrap().unwrap() {
            Incoming::Response { id, result } => match id.as_i64() {
                Some(1) => {
                    let r: InitializeResponse = decode(&what, result);
                    assert_eq!(
                        codexwire::user_agent_version(&r.user_agent),
                        Some(CODEX_VERSION)
                    );
                    assert_eq!(r.platform_family, "unix");
                }
                Some(2) => {
                    let r: ThreadStartResponse = decode(&what, result);
                    assert!(r.thread.turns.is_empty());
                }
                Some(3) => {
                    let _: TurnStartResponse = decode(&what, result);
                }
                _ => panic!("{what}: unexpected answer {id}"),
            },
            Incoming::Notification { method, params } => {
                let known = try_as::<ThreadStartedNotification>(&method, &params, |_| {})
                    || try_as::<TurnStartedNotification>(&method, &params, |_| {})
                    || try_as::<TurnCompletedNotification>(&method, &params, |n| {
                        assert!(n.turn.error.is_some(), "{what}: the turn failed (no login)");
                    })
                    || try_as::<ItemStartedNotification>(&method, &params, |n| {
                        known_item(&what, &n.item);
                    })
                    || try_as::<ItemCompletedNotification>(&method, &params, |n| {
                        known_item(&what, &n.item);
                    })
                    || try_as::<ErrorNotification>(&method, &params, |n| {
                        assert!(!n.error.message.is_empty());
                    });
                if known {
                    decoded += 1;
                } else {
                    other.push(method);
                }
            }
            m => panic!("{what}: unexpected {m:?}"),
        }
    }
    assert!(decoded >= 15, "decoded {decoded}");
    // The notifications codexwire has no type for yet.
    other.sort();
    other.dedup();
    assert_eq!(
        other,
        [
            "remoteControl/status/changed",
            "thread/status/changed",
            "warning"
        ]
    );
}

#[test]
fn an_item_of_a_newer_codex_is_unknown() {
    let v = serde_json::json!({"type": "brandNew", "id": "i1"});
    assert_eq!(
        serde_json::from_value::<ThreadItem>(v.clone()).unwrap(),
        ThreadItem::Unknown(v)
    );
    // A known tag whose body doesn't decode is Unknown too (serde's
    // untagged fallback): log Unknown items to see schema drift.
    let broken = serde_json::json!({"type": "agentMessage"});
    assert!(matches!(
        serde_json::from_value::<ThreadItem>(broken).unwrap(),
        ThreadItem::Unknown(_)
    ));
}
