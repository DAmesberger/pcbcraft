//! The pure search on synthetic declarations: its node budget holds on a
//! search space with no solution, and a 176-terminal part resolves.
use super::*;
use crate::functions::{
    Direction, Drive, Function, PinConfigs, Resource, ResourceRole, Role, SignalKind,
    TerminalDeclaration, TerminalFunctions, rules::CompiledRules,
};

fn function(id: u32, resource: u32, role: u32) -> Function {
    Function {
        id: FunctionId(id),
        label: format!("f{id}"),
        direction: Direction::Input,
        signal: SignalKind::Digital,
        drive: Drive::None,
        resource_role: (resource > 0).then_some(ResourceRole {
            resource: ResourceId(resource),
            role: RoleId(role),
        }),
        mux: None,
        source: None,
    }
}

/// `terminals` terminals, each with a GPIO default and one function per
/// role of every resource; `resources` resources with `roles` roles.
fn part(terminals: u32, resources: u32, roles: u32) -> (Vec<TerminalFunctions>, Vec<Resource>) {
    let mut next = 10_000;
    let mut declared = Vec::new();
    for _ in 0..terminals {
        let mut functions = vec![function(next, 0, 0)];
        let default = FunctionId(next);
        next += 1;
        for r in 0..resources {
            for k in 0..roles {
                functions.push(function(next, 1 + r, 1000 + r * 100 + k));
                next += 1;
            }
        }
        declared.push(TerminalFunctions {
            functions,
            design_default: Some(default),
            gpio: None,
            options: vec![],
        });
    }
    let resources = (0..resources)
        .map(|r| Resource {
            id: ResourceId(1 + r),
            label: format!("P{r}"),
            kind: None,
            roles: (0..roles)
                .map(|k| Role {
                    id: RoleId(1000 + r * 100 + k),
                    label: format!("r{k}"),
                    capacity: 1,
                    required_when_used: true,
                    signal: None,
                })
                .collect(),
        })
        .collect();
    (declared, resources)
}

fn run(terminals: u32, resources: u32, roles: u32) -> Result<SuggestReport> {
    let (declared, resources_list) = part(terminals, resources, roles);
    let names: Vec<String> = (0..terminals).map(|t| format!("T{t}")).collect();
    let declarations: Vec<TerminalDeclaration<'_>> = declared
        .iter()
        .zip(&names)
        .enumerate()
        .map(|(i, (f, name))| TerminalDeclaration {
            terminal: TerminalId(1 + i as u32),
            name,
            functions: Some(f),
        })
        .collect();
    let rules = CompiledRules::default();
    let selections = crate::functions::FunctionSelections::new();
    let configs = PinConfigs::new();
    let input = SuggestInput {
        terminals: &declarations,
        resources: &resources_list,
        constraints: &[],
        unavailable: None,
        rules: &rules,
        selections: &selections,
        configs: &configs,
        hints: &[],
    };
    let request = SuggestRequest {
        enable: resources_list
            .iter()
            .map(|r| PeripheralRequest {
                peripheral: r.id,
                roles: None,
            })
            .collect(),
        keep_existing: false,
        avoid: vec![],
        limit: Some(MAX_SUGGESTIONS),
    };
    suggest(&input, &request)
}

#[test]
fn the_node_budget_bounds_a_search_without_a_solution() -> Result<()> {
    // 3 peripherals × 3 roles = 9 roles over 8 terminals: no assignment
    // exists, and the space far exceeds the budget.
    let report = run(8, 3, 3)?;
    assert!(report.suggestions.is_empty());
    assert!(report.exhausted);
    assert_eq!(report.nodes, SUGGEST_NODE_BUDGET);
    assert_eq!(report.blockers.len(), 1);
    assert!(
        report.blockers[0]
            .message
            .contains("the search stopped after 100000 candidates")
    );
    Ok(())
}

#[test]
fn a_176_terminal_part_gets_ranked_assignments() -> Result<()> {
    let started = std::time::Instant::now();
    let report = run(176, 8, 2)?;
    let elapsed = started.elapsed();
    assert_eq!(report.suggestions.len(), MAX_SUGGESTIONS);
    assert!(!report.exhausted);
    // The first assignment takes the first 16 terminals in order.
    let first: Vec<u32> = report.suggestions[0].changed.iter().map(|t| t.0).collect();
    assert_eq!(first, (1..=16).collect::<Vec<_>>());
    // Measured, not asserted (the 50 ms figure is a target).
    eprintln!(
        "176-terminal suggestion: {} nodes in {elapsed:?}",
        report.nodes
    );
    Ok(())
}
