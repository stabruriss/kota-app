use super::*;
use crate::local_rpc::auth_fixture::Fixture;
use std::fs;

const KEY: &str = "fixture-jev-key";
fn states(settings: &JevSettings) -> Vec<(&str, &str)> { settings.keys.iter().map(|k| (k.provider_id.as_str(), k.status)).collect() }
fn verified(_: &Backend, _: &str, _: &str, _: &dyn Fn() -> bool) -> LocalResult<&'static str> { Ok("verified") }

#[test]
fn defaults_list_every_channel_without_creating_files() {
    let f = Fixture::new(json!({}));
    let settings = read(&f.backend.owned).unwrap();
    assert_eq!(settings, JevSettings { schema: 1, provider: "openrouter".into(), ready: false, keys: settings.keys.clone() });
    assert_eq!(states(&settings), PROVIDERS.iter().map(|p| (*p, "none")).collect::<Vec<_>>());
    assert!(!keys_path(&f.backend.owned).exists());
    assert_eq!(gate(&f.backend.owned).unwrap_err().code, "jev_key_required");
    assert!(current(&f.backend.owned).unwrap().is_none());
    assert_eq!(PROVIDERS.map(|p| model_id(p).unwrap()), ["~typesafe/jev-latest", "typesafe-ai/jev", "jev-1.13", "jev-latest"]);
}

#[test]
fn a_key_is_saved_only_after_jev_accepts_it_and_never_leaves_as_data() {
    let f = Fixture::new(json!({}));
    let owned = &f.backend.owned;
    let called = std::cell::Cell::new(0);
    let seen = std::cell::RefCell::new(Vec::new());
    let counting = |outcome: &'static str| { let (called, seen) = (&called, &seen); move |_: &Backend, provider: &str, key: &str, _: &dyn Fn() -> bool| {
        seen.borrow_mut().push((provider.to_owned(), key.to_owned())); called.set(called.get() + 1); Ok(outcome) } };
    for (key, code) in [("", "jev_key_empty"), ("!echo fixture", "jev_verification_failed"), ("$HOME", "jev_verification_failed")] {
        assert_eq!(save_key_with(&f.backend, "r-1", "typesafe", key, counting("verified")).unwrap_err().code, code);
    }
    assert_eq!(called.get(), 0);
    for outcome in ["rejected", "unavailable"] {
        assert_eq!(save_key_with(&f.backend, "r-1", "typesafe", KEY, counting(outcome)).unwrap_err().code, "jev_verification_failed");
        assert!(!keys_path(owned).exists());
    }
    assert_eq!(save_key_with(&f.backend, "r-1", "nope", KEY, counting("verified")).unwrap_err().code, "invalid_request");
    assert!(seen.borrow().iter().all(|entry| entry == &("typesafe".to_owned(), KEY.to_owned())));
    let settings = save_key_with(&f.backend, "r-2", "typesafe", KEY, counting("verified")).unwrap();
    assert!(states(&settings).contains(&("typesafe", "verified")));
    assert!(!settings.ready, "the selected channel is still openrouter");
    assert!(!serde_json::to_string(&settings).unwrap().contains(KEY));
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; assert_eq!(fs::metadata(keys_path(owned)).unwrap().permissions().mode() & 0o777, 0o600); }
    assert!(select(&f.backend, "typesafe").unwrap().ready);
    assert_eq!(select(&f.backend, "elsewhere").unwrap_err().code, "invalid_request");
    let credential = current(owned).unwrap().unwrap();
    assert_eq!((credential.provider.as_str(), credential.model_id, credential.key.as_str()), ("typesafe", "jev-latest", KEY));
    // A failed check keeps the saved key.
    assert_eq!(save_key_with(&f.backend, "r-3", "typesafe", "other-key", counting("rejected")).unwrap_err().code, "jev_verification_failed");
    assert_eq!(current(owned).unwrap().unwrap().key, KEY);
}

#[test]
fn a_rejection_marks_only_that_key_and_saving_again_clears_it() {
    let f = Fixture::new(json!({}));
    let owned = &f.backend.owned;
    select(&f.backend, "opencode").unwrap();
    save_key_with(&f.backend, "r-1", "opencode", KEY, verified).unwrap();
    let first = current(owned).unwrap().unwrap();
    mark_rejected(owned, &first).unwrap();
    assert_eq!(gate(owned).unwrap_err().code, "jev_key_rejected");
    assert!(states(&read(owned).unwrap()).contains(&("opencode", "rejected")));
    assert!(current(owned).unwrap().is_none());
    // The same key saved again is a new key record; the old marker no longer applies.
    save_key_with(&f.backend, "r-2", "opencode", KEY, verified).unwrap();
    assert!(gate(owned).is_ok());
    assert_ne!(current(owned).unwrap().unwrap().key_id, first.key_id);
    assert!(!rejected_path(owned, "opencode").exists());
}

#[test]
fn a_result_after_cancellation_is_discarded_and_one_check_runs_at_a_time() {
    let f = Fixture::new(json!({}));
    let cancelled = save_key_with(&f.backend, "r-cancel", "typesafe", KEY, |_, _, _, is_cancelled: &dyn Fn() -> bool| {
        assert!(!is_cancelled());
        cancel("someone-else").unwrap();
        assert!(!is_cancelled());
        cancel("r-cancel").unwrap();
        assert!(is_cancelled());
        Ok("verified")
    }).unwrap_err();
    assert_eq!(cancelled.code, "jev_verification_cancelled");
    assert!(!keys_path(&f.backend.owned).exists());
    let busy = save_key_with(&f.backend, "r-outer", "typesafe", KEY, |backend, _, _, _| {
        assert_eq!(save_key_with(backend, "r-inner", "typesafe", KEY, verified).unwrap_err().code, "busy-auth");
        Ok("verified")
    });
    assert!(busy.is_ok());
    assert!(save_key_with(&f.backend, "r-after", "typesafe", KEY, verified).is_ok());
}

#[test]
fn malformed_stores_fail_closed() {
    let f = Fixture::new(json!({}));
    let owned = &f.backend.owned;
    fs::create_dir_all(directory(owned)).unwrap();
    for bad in [json!({"schema":1,"provider":"unknown","keys":{}}), json!({"schema":2,"provider":"typesafe","keys":{}}),
        json!({"schema":1,"provider":"typesafe","keys":{"typesafe":{"key":""}}})] {
        fs::write(keys_path(owned), bad.to_string()).unwrap();
        assert_eq!(read(owned).unwrap_err().code, "jev_store_invalid");
        assert_eq!(gate(owned).unwrap_err().code, "jev_store_invalid");
    }
}
