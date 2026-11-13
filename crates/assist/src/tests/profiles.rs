//! Run profiles in the loop: Claude's keeps today's prompt and tools, and a
//! call refused again and again is named to the model, then ends the run.
use super::*;

#[test]
fn a_repeated_refused_call_is_named_then_ends_the_run() -> Fallible {
    let dir = tempfile::tempdir()?;
    let config = local_config(131_072)?;
    // Applying with nothing previewed is refused; the model sends it
    // unchanged, as a local model once did 54 times.
    let apply = || chat_call("a", "library_pinout_apply", r#"{"base": "12"}"#);
    let mut backend = openai(vec![apply(), apply(), apply(), apply()], &config);
    let mut session = profiled_session();
    let (outcome, log) = run_with(&config, &mut backend, text_datasheet(&dir)?, &mut session)?;
    assert_eq!(
        outcome.stop,
        Stop::Repeated("the model sent the same refused library_pinout_apply call 3 times".into()),
        "{log:?}"
    );
    assert_eq!(outcome.facts.requests, 3);
    let posts = &backend.wire.posts;
    let told = |n: usize| {
        posts[n].2["messages"]
            .as_array()
            .and_then(|m| m.last())
            .and_then(|m| m["content"].as_str())
            .is_some_and(|c| c.contains("was refused 2 times"))
    };
    // Not told after the first refusal; told after the second.
    assert!(!told(1) && told(2));
    assert!(outcome.facts.stop.contains("3 times"));
    Ok(())
}

#[test]
fn a_call_that_succeeds_between_refusals_starts_the_count_again() -> Fallible {
    // A tuning run was stopped wrongly: the same preview was refused, the
    // document fixed, the same preview refused for the next problem. A call
    // that succeeds in between means something may have changed.
    let dir = tempfile::tempdir()?;
    let config = local_config(131_072)?;
    let apply = || chat_call("a", "library_pinout_apply", r#"{"base": "12"}"#);
    let edit = chat_call("e", "library_pinout_edit", r#"{"format": 1}"#);
    let mut backend = openai(
        vec![apply(), edit, apply(), apply(), apply(), apply()],
        &config,
    );
    let mut session = profiled_session();
    let (outcome, log) = run_with(&config, &mut backend, text_datasheet(&dir)?, &mut session)?;
    assert!(matches!(outcome.stop, Stop::Repeated(_)), "{log:?}");
    // Three refusals after the edit, not two after it plus one before.
    assert_eq!(outcome.facts.requests, 5);
    Ok(())
}

#[test]
fn claude_runs_with_the_base_prompt_and_every_tool() -> Fallible {
    let dir = tempfile::tempdir()?;
    let config = shipped_config()?;
    assert_eq!(config.run_profile.name, "claude");
    let mut backend = anthropic(script()?)?;
    let mut session = profiled_session();
    let (outcome, _) = run_with(&config, &mut backend, datasheet(&dir)?, &mut session)?;
    assert_eq!(outcome.facts.run_profile, "claude");
    let body = bodies(&backend)[0];
    assert_eq!(body["system"][0]["text"], config.base_prompt);
    assert_eq!(config.base_prompt, crate::AssistLibrary::locate()?.system);
    // The shipped profile declares the server fallback for its models.
    assert_eq!(body["fallbacks"], "default");
    let names: Vec<&str> = body["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    let all = crate::tools::definitions();
    assert_eq!(
        names,
        all.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()
    );
    Ok(())
}
