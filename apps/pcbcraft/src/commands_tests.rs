use super::*;
use egui::Key as K;

fn ids<'a>(commands: impl IntoIterator<Item = &'a Command>) -> Vec<&'a str> {
    commands.into_iter().map(|c| c.id.as_str()).collect()
}

/// No two commands share a chord where both could see it: the same
/// context, or one inside the other without a tool mode between them.
#[test]
fn the_default_keymap_has_no_conflicts() {
    let registry = Registry::standard();
    assert_eq!(registry.conflicts(), vec![]);
}

#[test]
fn ids_are_unique_and_titles_and_icons_are_set() {
    let registry = Registry::standard();
    let mut seen = std::collections::BTreeSet::new();
    for command in registry.commands() {
        assert!(seen.insert(command.id.clone()), "{} twice", command.id);
        assert!(!command.title.is_empty(), "{}", command.id);
        assert!(
            !solidcraft_ui_egui::icon_svg::resolve(command.icon).placeholder,
            "{}: icon {}",
            command.id,
            command.icon
        );
    }
}

/// A conflict in overlapping contexts is found; separate workspaces and an
/// exclusive mode hiding its view are not conflicts (an ordinary mode needs
/// a SHADOWS entry: `an_undeclared_mode_override_is_a_conflict`).
#[test]
fn conflicts_are_detected_by_context_overlap() {
    let command = |id: &str, context, chord| {
        Command::new(id, id, context, Action::Key)
            .chord(chord)
            .in_place()
    };
    let mut registry = Registry {
        commands: vec![
            command("a", Context::PcbLayout, Chord::key(K::Q)),
            command("b", Context::Schematic, Chord::key(K::Q)),
            command("c", Context::BoardShape, Chord::key(K::Q)),
        ],
        custom: BTreeMap::new(),
    };
    assert_eq!(registry.conflicts(), vec![]);
    registry
        .commands
        .push(command("d", Context::Pcb, Chord::key(K::Q)));
    registry
        .commands
        .push(command("e", Context::Global, Chord::cmd(K::S)));
    registry
        .commands
        .push(command("f", Context::Symbol, Chord::cmd(K::S)));
    let found: Vec<(String, String)> = registry
        .conflicts()
        .into_iter()
        .map(|c| (c.first, c.second))
        .collect();
    assert_eq!(
        found,
        vec![("a".into(), "d".into()), ("e".into(), "f".into())]
    );
}

#[test]
fn rebinding_refuses_a_taken_key_and_resets_to_the_default() -> Result<(), String> {
    let mut registry = Registry::standard();
    let route = registry.get("pcb.route").cloned().ok_or("route")?;
    // V is the Via tool's in the same context.
    assert!(
        registry
            .rebind("pcb.route", Some(Chord::key(K::V)))
            .is_err()
    );
    registry.rebind("pcb.route", Some(Chord::key(K::K)))?;
    assert_eq!(registry.chord(&route), Some(Chord::key(K::K)));
    let active = active_contexts(Context::PcbLayout);
    assert_eq!(
        registry
            .resolve(Chord::key(K::K), &active)
            .map(|c| c.id.as_str()),
        Some("pcb.route")
    );
    assert!(registry.resolve(Chord::key(K::R), &active).is_none());
    registry.rebind("pcb.route", Some(Chord::key(K::R)))?;
    assert!(registry.custom.is_empty(), "the default is not stored");
    registry.rebind("pcb.route", None)?;
    assert_eq!(registry.shortcut(&route), None);
    assert!(registry.rebind("no.such", None).is_err());
    Ok(())
}

/// Lookup walks from the innermost context out: a tool mode's key wins
/// over its view's, the board shape sketch hides the Layout keys but not
/// the Everywhere ones, and another workspace's keys never apply.
#[test]
fn resolve_follows_the_active_contexts() {
    let registry = Registry::standard();
    let id = |context, chord| {
        registry
            .resolve(chord, &active_contexts(context))
            .map(|c| c.id.clone())
    };
    let s = |id: &str| Some(id.to_owned());
    assert_eq!(id(Context::PcbLayout, Chord::key(K::R)), s("pcb.route"));
    assert_eq!(id(Context::PcbLayout, Chord::key(K::X)), None);
    assert_eq!(id(Context::PcbLayout, Chord::key(K::V)), s("pcb.via"));
    // Routing: the Route tool's keys (`ROUTE_KEYS`) win over Layout's and
    // the Everywhere ones while it is armed; R still names Route.
    let in_route = |chord| {
        registry
            .resolve(chord, &active_contexts(Context::PcbRoute))
            .map(|c| (c.context, c.handler))
    };
    for chord in [
        Chord::key(K::V),
        Chord::key(K::V).with_shift(),
        Chord::key(K::F),
        Chord::key(K::D),
        Chord::key(K::D).with_shift(),
        Chord::key(K::W),
        Chord::key(K::Slash),
        Chord::key(K::Backspace),
        Chord::key(K::Escape),
        Chord::key(K::Num1),
        Chord::key(K::Num9),
    ] {
        assert_eq!(
            in_route(chord),
            Some((Context::PcbRoute, Handler::InPlace)),
            "{chord:?}"
        );
    }
    assert_eq!(id(Context::PcbRoute, Chord::key(K::R)), s("pcb.route"));
    assert_eq!(
        id(Context::PcbRoute, Chord::cmd(K::Num1)),
        s("workspace.component")
    );
    // Outside Route the same keys keep their meaning.
    assert_eq!(id(Context::PcbLayout, Chord::key(K::D)), s("pcb.toggle-3d"));
    assert_eq!(id(Context::PcbLayout, Chord::key(K::Num1)), None);
    assert_eq!(id(Context::PcbFill, Chord::key(K::F)), s("view.fit"));
    // The board shape sketch: X is construction, V nothing, ⌘S still saves.
    assert_eq!(
        id(Context::BoardShape, Chord::key(K::X)),
        s("shape.construction")
    );
    assert_eq!(id(Context::BoardShape, Chord::key(K::V)), None);
    assert_eq!(id(Context::BoardShape, Chord::cmd(K::S)), s("file.save"));
    assert_eq!(
        id(Context::BoardShape, Chord::key(K::F)),
        s("shape.bs-fillet")
    );
    assert_eq!(id(Context::PcbLayout, Chord::key(K::F)), s("view.fit"));
    // Workspaces stay apart.
    assert_eq!(
        id(Context::Schematic, Chord::key(K::P)),
        s("schematic.place")
    );
    assert_eq!(id(Context::PcbLayout, Chord::key(K::P)), s("pcb.fill"));
    assert_eq!(id(Context::Schematic, Chord::key(K::X)), None);
    assert_eq!(id(Context::Pcb3d, Chord::key(K::X)), None);
    assert_eq!(id(Context::Pcb3d, Chord::key(K::D)), s("pcb.toggle-3d"));
    // Modifiers must match exactly.
    assert_eq!(id(Context::Symbol, Chord::key(K::L)), s("symbol.draw-line"));
    assert_eq!(
        id(Context::Symbol, Chord::key(K::L).with_shift()),
        s("symbol.draw-polyline")
    );
    assert_eq!(id(Context::Global, Chord::cmd(K::Z)), s("edit.undo"));
    assert_eq!(
        id(Context::Global, Chord::cmd(K::Z).with_shift()),
        s("edit.redo")
    );
}

/// The editors' tool tables reach the registry with the keys their
/// canvases handle, and the ribbon finds each tool's command.
#[test]
fn tool_tables_are_listed_with_their_keys() -> Result<(), String> {
    let registry = Registry::standard();
    let line = registry
        .for_tool(Workspace::Part, "draw-line")
        .ok_or("line")?;
    assert_eq!(registry.chord(line), Some(Chord::key(K::L)));
    assert_eq!(line.handler, Handler::InPlace);
    let slot = registry
        .for_tool(Workspace::Part, "fp-slot")
        .ok_or("slot")?;
    assert_eq!(registry.chord(slot), Some(Chord::key(K::H).with_shift()));
    let hole = registry.for_tool(Workspace::Pcb, "bs-hole").ok_or("hole")?;
    assert_eq!(hole.context, Context::BoardShape);
    let route = registry.for_tool(Workspace::Pcb, "route").ok_or("route")?;
    assert_eq!(registry.shortcut(route).as_deref(), Some("R"));
    assert_eq!(route.handler, Handler::Registry);
    assert!(registry.for_tool(Workspace::Schematic, "route").is_none());
    let save = registry.for_app(AppCommand::Save).ok_or("save")?;
    assert_eq!(registry.chord(save), Some(Chord::cmd(K::S)));
    Ok(())
}

/// A key table such as the Route tool's is shown in place; a key it
/// cannot read is kept as text.
#[test]
fn key_tables_add_display_only_entries() {
    let registry = Registry::default().with_key_table(
        Context::PcbRoute,
        &[
            ("W", "Next width"),
            ("⇧W", "Previous width"),
            ("Wheel", "Zoom"),
        ],
    );
    let entries = registry.commands();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].chord, Some(Chord::key(K::W)));
    assert_eq!(entries[1].chord, Some(Chord::key(K::W).with_shift()));
    assert_eq!(entries[2].chord, None);
    assert_eq!(registry.shortcut(&entries[2]).as_deref(), Some("Wheel"));
    assert!(
        entries
            .iter()
            .all(|c| !c.runnable() && c.handler == Handler::InPlace)
    );
    // Display-only entries never reach the palette.
    assert!(registry.search("width", &[], &[]).is_empty());
}

/// The Route tool's real table: compound cells become several chords,
/// shown as one cell; "Click" is text.
#[test]
fn the_route_tool_table_is_listed_with_every_key() -> Result<(), String> {
    let registry = Registry::standard();
    let rows: Vec<&Command> = registry
        .commands()
        .iter()
        .filter(|c| c.id.starts_with("keys.pcbroute."))
        .collect();
    assert_eq!(rows.len(), crate::pcb_editor::ROUTE_KEYS.len());
    let row = |key: egui::Key| {
        rows.iter()
            .find(|c| registry.chords(c).contains(&Chord::key(key)))
            .copied()
    };
    let layers = row(K::Num5).ok_or("1 to 9")?;
    assert_eq!(registry.chords(layers).len(), 9);
    assert_eq!(registry.shortcut(layers).as_deref(), Some("1–9"));
    let via = row(K::V).ok_or("V")?;
    assert!(
        registry
            .chords(via)
            .contains(&Chord::key(K::V).with_shift())
    );
    assert_eq!(
        registry.shortcut(via),
        Some(format!("V, {}", Chord::key(K::V).with_shift().shown()))
    );
    assert!(
        rows.iter()
            .any(|c| c.chord.is_none() && registry.shortcut(c).as_deref() == Some("Click"))
    );
    Ok(())
}

#[test]
fn key_cells_read_lists_and_ranges() {
    let (chords, shown) = key_cell("W, Shift+W").unwrap_or_default();
    assert_eq!(
        chords,
        vec![Chord::key(K::W), Chord::key(K::W).with_shift()]
    );
    assert_eq!(
        shown,
        format!("W, {}", Chord::key(K::W).with_shift().shown())
    );
    let (chords, shown) = key_cell("1 to 3").unwrap_or_default();
    assert_eq!(
        chords,
        vec![
            Chord::key(K::Num1),
            Chord::key(K::Num2),
            Chord::key(K::Num3)
        ]
    );
    assert_eq!(shown, "1–3");
    assert_eq!(key_cell("Enter or Esc").map(|c| c.0.len()), Some(2));
    assert_eq!(key_cell("/").map(|c| c.0), Some(vec![Chord::key(K::Slash)]));
    assert_eq!(key_cell("Click"), None);
    assert_eq!(key_cell("9 to 1"), None);
    assert_eq!(key_cell(""), None);
}

/// Every shadow names a key its mode really takes from a command that
/// really has it there; otherwise it is stale.
#[test]
fn every_shadow_is_used() {
    let registry = Registry::standard();
    for (mode, chord, id) in SHADOWS {
        assert!(mode.modal(), "{mode:?}");
        let outer = registry.get(id);
        assert!(
            outer.is_some_and(|c| mode.within(c.context)
                && c.context != *mode
                && registry.chords(c).contains(chord)),
            "{mode:?} {chord:?} {id}"
        );
        assert!(
            registry
                .commands()
                .iter()
                .any(|c| c.context == *mode && registry.chords(c).contains(chord)),
            "{mode:?} takes no {chord:?}"
        );
    }
}

/// A tool mode taking a key from around it without a shadow entry is a
/// conflict, as is one taking a different key than the entry names.
#[test]
fn an_undeclared_mode_override_is_a_conflict() {
    let mut registry = Registry::standard();
    // B (Board shape) is not shadowed for the Route tool.
    registry.commands.push(
        Command::new("test.route-b", "B in Route", Context::PcbRoute, Action::Key)
            .chord(Chord::key(K::B))
            .in_place(),
    );
    let found: Vec<String> = registry.conflicts().into_iter().map(|c| c.first).collect();
    assert_eq!(found, vec!["pcb.board-shape".to_owned()]);
    // The board shape sketch's own keys never clash with Layout's.
    let shape = Registry::standard();
    assert!(
        shape
            .conflicts()
            .iter()
            .all(|c| !c.first.starts_with("shape.") && !c.second.starts_with("shape."))
    );
}

#[test]
fn chords_parse_and_display_per_platform() {
    let save_as = Chord::cmd(K::S).with_shift();
    assert_eq!(save_as.display(true), "⇧⌘S");
    assert_eq!(save_as.display(false), "Ctrl+Shift+S");
    assert_eq!(save_as.name(), "Ctrl+Shift+S");
    assert_eq!(Chord::parse("Ctrl+Shift+S"), Some(save_as));
    assert_eq!(Chord::parse("⇧⌘S"), Some(save_as));
    assert_eq!(Chord::parse("Shift+V"), Some(Chord::key(K::V).with_shift()));
    assert_eq!(Chord::parse("Esc"), Some(Chord::key(K::Escape)));
    assert_eq!(Chord::parse("Backspace"), Some(Chord::key(K::Backspace)));
    assert_eq!(Chord::parse("Hyper+V"), None);
    assert_eq!(Chord::parse(""), None);
    assert_eq!(Chord::key(K::Escape).display(true), "Esc");
    assert_eq!(Chord::cmd(K::Slash).display(true), "⌘/");
    assert_eq!(Chord::cmd(K::OpenBracket).display(false), "Ctrl+[");
    assert_eq!(Chord::key(K::ArrowUp).display(false), "Up");
    // Every default chord survives its canonical name.
    for command in Registry::standard().commands() {
        if let Some(chord) = command.chord {
            assert_eq!(Chord::parse(&chord.name()), Some(chord), "{}", command.id);
        }
    }
}

/// A title prefix beats a word start beats a keyword beats letters in
/// order; equal matches put the active contexts first.
#[test]
fn search_ranks_titles_then_words_then_scattered_letters() {
    let registry = Registry::standard();
    let active = active_contexts(Context::PcbLayout);
    let first = |q: &str| {
        registry
            .search(q, &[], &active)
            .first()
            .map(|c| c.id.clone())
    };
    assert_eq!(first("route").as_deref(), Some("pcb.route"));
    assert_eq!(first("save as").as_deref(), Some("file.save-as"));
    // A keyword: "pour" finds Fill.
    assert_eq!(first("pour").as_deref(), Some("pcb.fill"));
    // Scattered letters still find the command.
    assert_eq!(first("kbdshrt").as_deref(), Some("app.shortcuts"));
    let hits = ids(registry.search("sav", &[], &active));
    let save = hits.iter().position(|id| *id == "file.save");
    let save_as = hits.iter().position(|id| *id == "file.save-as");
    assert!(save < save_as, "{hits:?}");
    // "Select" is in the Layout context: its PCB command leads.
    let select = ids(registry.search("select", &[], &active));
    assert_eq!(select.first().copied(), Some("pcb.select"));
    let select = ids(registry.search("select", &[], &active_contexts(Context::Schematic)));
    assert_eq!(select.first().copied(), Some("schematic.select"));
    assert!(registry.search("zzzzqq", &[], &active).is_empty());
}

#[test]
fn an_empty_search_lists_recent_commands_first() {
    let registry = Registry::standard();
    let recent = vec!["pcb.via".to_owned(), "file.export".to_owned()];
    let hits = ids(registry.search("", &recent, &active_contexts(Context::PcbLayout)));
    assert_eq!(hits.get(..2), Some(&["pcb.via", "file.export"][..]));
    // Then the commands in force here before the rest.
    let third = registry.get(hits[2]).map(|c| c.context);
    assert!(third.is_some_and(|c| active_contexts(Context::PcbLayout).contains(&c)));
}

#[test]
fn contexts_nest() {
    assert_eq!(
        active_contexts(Context::PcbRoute),
        vec![
            Context::PcbRoute,
            Context::PcbLayout,
            Context::Pcb,
            Context::Global
        ]
    );
    assert!(Context::Symbol.within(Context::Component));
    assert!(!Context::Symbol.within(Context::Pcb));
    for context in Context::ALL {
        assert!(context.within(Context::Global));
        assert!(!context.label().is_empty());
    }
}
