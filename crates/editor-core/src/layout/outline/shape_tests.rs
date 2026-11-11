//! Slice S1 without a sketch evaluator: the `board.outline_fit` design rule
//! (docs/board-sketch-references.md §5.1). The closed-loop tests that need
//! the sketch solver live in `pcbcraft_solidcraft::board::closed_loop_tests`.
use crate::rules::{ProjectRules, RuleGroup, RuleKey, RuleValue};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// `board.outline_fit` is a board-edge rule (key `board_outline_fit`)
/// the profile sets (the test prototype: the 0.01 mm chord tolerance the
/// board solve used before); without a profile it is unset.
#[test]
fn the_outline_fit_rule_comes_from_the_profile() -> Fallible<()> {
    let key: RuleKey = serde_json::from_value(serde_json::json!({"rule": "board_outline_fit"}))?;
    assert_eq!(key.group(), RuleGroup::BoardEdge);
    let rules = crate::rules::fixtures::rules()?;
    assert_eq!(rules.resolve(key).value, RuleValue::Length(0.01));
    assert_eq!(
        ProjectRules::default().resolve(key).value,
        RuleValue::Length(0.0)
    );
    assert!(RuleKey::all().contains(&key));
    Ok(())
}
