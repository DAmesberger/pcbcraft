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

/// Follow net is a PCB 3D command in the palette with ⇧N, which the
/// footprint tab's ⇧N (pad row) and Layout do not reach.
#[test]
fn follow_net_is_a_pcb_3d_command_on_shift_n() -> Result<(), String> {
    let registry = Registry::standard();
    let command = registry
        .for_app(AppCommand::ToggleFollowNet)
        .ok_or("no follow-net command")?;
    assert_eq!(command.id, "view.follow-net");
    assert_eq!(command.context, Context::Pcb3d);
    assert!(command.runnable(), "listed in the palette");
    let chord = Chord::key(K::N).with_shift();
    assert_eq!(registry.chords(command), vec![chord]);
    let resolved = |context| {
        registry
            .resolve(chord, &active_contexts(context))
            .map(|c| c.id.clone())
    };
    assert_eq!(resolved(Context::Pcb3d).as_deref(), Some("view.follow-net"));
    assert_ne!(
        resolved(Context::PcbLayout).as_deref(),
        Some("view.follow-net")
    );
    assert_ne!(
        resolved(Context::Footprint).as_deref(),
        Some("view.follow-net")
    );
    Ok(())
}

#[test]
fn ids_are_unique_and_titles_and_icons_are_set() {
    let registry = Registry::standard();
    let mut seen = std::collections::BTreeSet::new();
    for command in registry.commands() {
        assert!(seen.insert(command.id.clone()), "{} twice", command.id);
        assert!(!command.title.is_empty(), "{}", command.id);
        assert!(
            !pcbcraft_ui::icon_svg::resolve(command.icon).placeholder,
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
    // The layer digits are Layout's in every tool, never the Route tool's.
    for context in [
        Context::PcbLayout,
        Context::PcbRoute,
        Context::PcbFill,
        Context::PcbTune,
    ] {
        assert_eq!(id(context, Chord::key(K::Num1)), s("pcb.layer-1"));
        assert_eq!(id(context, Chord::key(K::Num0)), s("pcb.layer-10"));
    }
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
    let registry = Registry::default().with_keys(
        Context::PcbRoute,
        &[
            KeyDecl::new("t.next", "W", "Next width"),
            KeyDecl::new("t.previous", "⇧W", "Previous width"),
            KeyDecl::new("t.zoom", "Wheel", "Zoom"),
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

/// The Route tool's declared keys are commands of their own: each
/// direction of a cycle is its own key, the layer digits are nine
/// commands, F flips with or without Shift, and "Click" is text that
/// cannot be bound.
#[test]
fn the_route_tool_keys_are_commands() -> Result<(), String> {
    let registry = Registry::standard();
    let rows: Vec<&Command> = registry
        .commands()
        .iter()
        .filter(|c| c.context == Context::PcbRoute)
        .collect();
    assert_eq!(rows.len(), crate::pcb_editor::ROUTE_KEYS.len());
    let chords = |id: &str| registry.get(id).map(|c| registry.chords(c));
    assert_eq!(chords("route.width-next"), Some(vec![Chord::key(K::W)]));
    assert_eq!(
        chords("route.width-previous"),
        Some(vec![Chord::key(K::W).with_shift()])
    );
    assert!(registry.get("route.layer-5").is_none());
    assert_eq!(
        chords("route.flip"),
        Some(vec![Chord::key(K::F), Chord::key(K::F).with_shift()])
    );
    let click = registry.get("route.click").ok_or("click")?;
    assert_eq!(registry.shortcut(click).as_deref(), Some("Click"));
    assert!(!Registry::bindable(click));
    let mut rebound = registry.clone();
    assert!(
        rebound
            .rebind("route.click", Some(Chord::key(K::K)))
            .is_err()
    );
    Ok(())
}

/// A rebinding moves the key everywhere the registry is read: lookup,
/// the shown key and the shortcuts sheet; Reset gives the default back.
#[test]
fn a_rebinding_reaches_lookup_display_and_the_sheet() -> Result<(), String> {
    let mut registry = Registry::standard();
    registry.rebind("route.width-next", Some(Chord::key(K::Q)))?;
    let active = active_contexts(Context::PcbRoute);
    let id = |registry: &Registry, chord| registry.resolve(chord, &active).map(|c| c.id.clone());
    assert_eq!(
        id(&registry, Chord::key(K::Q)).as_deref(),
        Some("route.width-next")
    );
    assert_eq!(id(&registry, Chord::key(K::W)), None);
    let command = registry.get("route.width-next").ok_or("width")?;
    assert_eq!(registry.shortcut(command).as_deref(), Some("Q"));
    assert!(registry.is_custom("route.width-next"));
    let sheet = crate::app::sheet_rows(&registry, "next track width", &active);
    assert_eq!(sheet, vec![("Next track width".to_owned(), "Q".to_owned())]);
    registry.reset("route.width-next");
    assert_eq!(
        id(&registry, Chord::key(K::W)).as_deref(),
        Some("route.width-next")
    );
    Ok(())
}

/// A key another command holds is refused with that command's name; Swap
/// trades the two keys as one change, and is refused (changing nothing)
/// when the freed key would clash with a third command.
#[test]
fn conflicts_are_refused_or_swapped() -> Result<(), String> {
    let mut registry = Registry::standard();
    let refused = registry.rebind("pcb.tune", Some(Chord::key(K::B)));
    assert!(refused.is_err_and(|why| why.contains("Board shape")));
    assert_eq!(
        registry
            .conflict_for("pcb.tune", Chord::key(K::B))
            .map(|c| c.id.as_str()),
        Some("pcb.board-shape")
    );
    // Route's V shadows the Via tool only: Route cannot take V.
    assert!(
        registry
            .rebind("pcb.route", Some(Chord::key(K::V)))
            .is_err()
    );
    registry.swap("pcb.tune", Chord::key(K::B))?;
    let chord = |r: &Registry, id: &str| r.get(id).and_then(|c| r.chord(c));
    assert_eq!(chord(&registry, "pcb.tune"), Some(Chord::key(K::B)));
    assert_eq!(chord(&registry, "pcb.board-shape"), Some(Chord::key(K::T)));
    assert_eq!(registry.conflicts(), vec![]);
    // Wire taking Fit view's F is refused, and nothing changes. The Wire
    // tool's own F (flip the elbow) now holds F inside the Schematic, so
    // that is the clash named; before the tool had keys it was the Route
    // tool's W that Fit view would have been handed.
    let mut three = Registry::standard();
    assert!(
        three
            .swap("schematic.wire", Chord::key(K::F))
            .is_err_and(|why| why.contains("Flip the elbow corner"))
    );
    assert!(three.overrides().is_empty());
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
    // A workspace's name leads to it, and a whole word beats a word that
    // only starts with the query (night round 4 Q4).
    assert_eq!(first("schematic").as_deref(), Some("workspace.schematic"));
    assert_eq!(first("component").as_deref(), Some("workspace.component"));
    assert_eq!(first("pcb").as_deref(), Some("workspace.pcb"));
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

/// Digits mean a standard view in PCB 3D and a via layer while routing in
/// Layout: sibling contexts, so neither sees the other's keys. ⇧F frames
/// the selection in 3D and views from the bottom in Layout. A rebound view
/// key moves; the 3D gestures are listed but take no key.
#[test]
fn view_keys_and_route_digits_resolve_by_context() -> Result<(), String> {
    let registry = Registry::standard();
    let id = |context, chord| {
        registry
            .resolve(chord, &active_contexts(context))
            .map(|c| c.id.clone())
    };
    assert_eq!(
        id(Context::Pcb3d, Chord::key(K::Num1)).as_deref(),
        Some("view.front")
    );
    assert_eq!(
        id(Context::Pcb3d, Chord::key(K::Num7)).as_deref(),
        Some("view.iso")
    );
    assert_eq!(
        id(Context::PcbRoute, Chord::key(K::Num1)).as_deref(),
        Some("pcb.layer-1")
    );
    assert_eq!(
        id(Context::PcbLayout, Chord::key(K::Num7)).as_deref(),
        Some("pcb.layer-7")
    );
    assert_eq!(
        id(Context::PcbLayout, Chord::key(K::Num0)).as_deref(),
        Some("pcb.layer-10")
    );
    // 3D keeps its 1-7 views; its 8, 9 and 0 are free (the layer keys are
    // Layout's, a sibling context).
    for key in [K::Num8, K::Num9, K::Num0] {
        assert_eq!(id(Context::Pcb3d, Chord::key(key)), None);
    }
    // The board shape sketch hides them.
    assert_eq!(id(Context::BoardShape, Chord::key(K::Num1)), None);
    let shift_f = Chord::key(K::F).with_shift();
    assert_eq!(
        id(Context::Pcb3d, shift_f).as_deref(),
        Some("view.selection")
    );
    assert_eq!(
        id(Context::PcbLayout, shift_f).as_deref(),
        Some("pcb.from-bottom")
    );
    assert_eq!(
        id(Context::PcbRoute, shift_f).as_deref(),
        Some("route.flip")
    );
    let mut rebound = registry.clone();
    rebound.rebind("view.top", Some(Chord::key(K::T)))?;
    assert_eq!(
        rebound
            .resolve(Chord::key(K::T), &active_contexts(Context::Pcb3d))
            .map(|c| c.id.as_str()),
        Some("view.top")
    );
    let gestures: Vec<&Command> = registry
        .commands()
        .iter()
        .filter(|c| c.id.starts_with("gesture.pcb3d."))
        .collect();
    assert_eq!(gestures.len(), crate::navigation::NAVIGATION_3D.len());
    assert!(
        gestures
            .iter()
            .all(|c| !Registry::bindable(c) && registry.chords(c).is_empty())
    );
    Ok(())
}

/// The ten layer keys are app-wide, rebindable commands with the default
/// digits 1 to 9 and 0; the copper toggles cover every layer a stackup can
/// have, and a command names its stackup layer.
#[test]
fn the_layer_keys_and_copper_toggles_cover_the_stackup() -> Result<(), String> {
    use crate::commands::StackupLayer;
    let mut registry = Registry::standard();
    let digits = [
        K::Num1,
        K::Num2,
        K::Num3,
        K::Num4,
        K::Num5,
        K::Num6,
        K::Num7,
        K::Num8,
        K::Num9,
        K::Num0,
    ];
    for (index, key) in digits.into_iter().enumerate() {
        let id = format!("pcb.layer-{}", index + 1);
        let command = registry.get(&id).ok_or_else(|| id.clone())?;
        assert_eq!(command.chord, Some(Chord::key(key)), "{id}");
        assert_eq!(command.handler, Handler::Registry);
        assert!(command.runnable());
        assert_eq!(command.stackup_layer(), Some(StackupLayer::Place(index)));
    }
    registry.rebind("pcb.layer-3", Some(Chord::key(K::Q).with_shift()))?;
    assert_eq!(
        registry
            .resolve(
                Chord::key(K::Q).with_shift(),
                &active_contexts(Context::PcbRoute)
            )
            .map(|c| c.id.as_str()),
        Some("pcb.layer-3")
    );
    let toggles = registry
        .commands()
        .iter()
        .filter(|c| matches!(c.stackup_layer(), Some(StackupLayer::Named(_))))
        .count();
    assert_eq!(toggles, pcbcraft_core::MAX_COPPER_LAYERS);
    let inner = registry
        .get("pcb.layer.copper-63")
        .and_then(|c| c.stackup_layer())
        .ok_or("the last inner toggle")?;
    assert_eq!(
        inner,
        StackupLayer::Named(pcbcraft_core::CopperLayer::Inner(62))
    );
    let four = pcbcraft_core::Stackup::preset(4)
        .map_err(|e| e.to_string())?
        .layers();
    assert!(StackupLayer::Place(3).on(&four));
    assert!(!StackupLayer::Place(4).on(&four));
    assert!(!inner.on(&four));
    assert!(
        registry.conflicts().is_empty(),
        "{:?}",
        registry.conflicts()
    );
    Ok(())
}

/// A menu row shows one key where a command has two; the palette and the
/// Keyboard shortcuts sheet still list both. Every delete command is so.
#[test]
fn menu_rows_show_one_key_of_a_command_with_two() -> Result<(), String> {
    let registry = Registry::standard();
    let delete = Chord::key(egui::Key::Delete).shown();
    let backspace = Chord::key(egui::Key::Backspace).shown();
    for id in [
        "schematic.delete",
        "pcb.delete",
        "footprint.delete",
        "shape.delete",
    ] {
        let command = registry.get(id).ok_or(id)?;
        assert_eq!(
            registry.menu_shortcut(command),
            Some(delete.clone()),
            "{id}"
        );
        assert_eq!(
            registry.shortcut(command),
            Some(format!("{delete}, {backspace}")),
            "{id}"
        );
    }
    // One key: as ever. No key: none.
    let save = registry.get("file.save").ok_or("save")?;
    assert_eq!(registry.menu_shortcut(save), registry.shortcut(save));
    // The user's own key replaces both.
    let mut custom = Registry::standard();
    custom
        .rebind("pcb.delete", Some(Chord::key(egui::Key::K)))
        .map_err(|e| e.to_string())?;
    let command = custom.get("pcb.delete").ok_or("pcb.delete")?;
    assert_eq!(
        custom.menu_shortcut(command),
        Some(Chord::key(egui::Key::K).shown())
    );
    Ok(())
}

/// The shortcuts sheet has one row per name in a context: the palette's
/// two keys share it (N7).
#[test]
fn the_shortcut_sheet_lists_a_name_once_with_all_its_keys() {
    let registry = Registry::standard();
    let rows = crate::app::sheet_rows(&registry, "Command palette", &[Context::Global]);
    let shown: Vec<_> = rows
        .iter()
        .filter(|(n, _)| n == "Command palette")
        .collect();
    let both = format!(
        "{}, {}",
        Chord::cmd(egui::Key::P).shown(),
        Chord::cmd(egui::Key::P).with_shift().shown()
    );
    assert_eq!(shown.len(), 1, "{rows:?}");
    assert_eq!(shown[0].1, both);
}
