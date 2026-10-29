use orch_core::{
    model::{LogEntry, Secret},
    sample_data::sample_state,
};

use super::{panels::wrap, *};

fn text(grid: &Grid) -> String {
    (0..grid.rows)
        .map(|y| grid.row(y).iter().map(|c| c.ch).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn room() -> Room {
    let mut room = Room::new();
    room.set_state(sample_state());
    room
}

#[test]
fn composes_every_view_at_many_sizes() {
    let mut room = room();
    for view in View::ALL {
        room.view = view;
        for (cols, rows) in [(MIN_COLS, MIN_ROWS), (120, 40), (240, 70), (40, 10)] {
            room.compose(cols, rows);
        }
    }
}

#[test]
fn overview_shows_the_control_room() {
    let mut room = room();
    // Put the permission modal off to see the board behind it.
    room.key(Key::Escape);
    let screen = text(&room.compose(140, 44));
    for expected in [
        "╔═ ORCH//SHOP ═",
        "SECTOR MAP",
        "PERMISSION QUEUE (1)",
        "EVENT LOG",
        "AGENT // TESTS",
        "VOICE // PUSH-TO-TALK",
        "Bash: cargo test",
        "CAUTION",
        "::ALERT::",
    ] {
        assert!(
            screen.contains(expected),
            "missing {expected:?} in\n{screen}"
        );
    }
    assert!(screen.starts_with('╔') && screen.ends_with('╝'));
}

#[test]
fn keys_switch_views_and_selection() {
    let mut room = room();
    room.key(Key::Function(2));
    assert_eq!(room.view, View::Agents);
    let before = room.selected;
    room.key(Key::Char('j'));
    assert_ne!(room.selected, before);
    let sent = room.drain();
    assert!(
        sent.iter()
            .any(|c| matches!(c, Command::SaveLayout(layout) if layout.contains("agents")))
    );
    assert!(sent.iter().any(|c| matches!(c, Command::Select(Some(_)))));
}

fn type_line(room: &mut Room, text: &str) {
    room.key(Key::Enter);
    for ch in text.chars() {
        room.key(Key::Char(ch));
    }
    room.key(Key::Enter);
}

#[test]
fn plain_text_talks_and_colon_runs_commands() {
    let mut room = room();
    room.drain();
    type_line(&mut room, "rode os testes");
    assert!(
        matches!(&room.drain()[..], [Command::Say { text, selected: Some(_) }] if text == "rode os testes")
    );

    type_line(
        &mut room,
        ":new API Tests opencode: write the /pix tests | tests only, never edits src/",
    );
    assert_eq!(
        room.drain(),
        vec![Command::CreateAgent {
            group: "API".into(),
            name: "Tests".into(),
            provider: Some("opencode".into()),
            task: "write the /pix tests".into(),
            persona: "tests only, never edits src/".into(),
        }]
    );

    type_line(&mut room, ":group Front: checkout screen");
    assert_eq!(
        room.drain(),
        vec![Command::CreateGroup {
            name: "Front".into(),
            goal: "checkout screen".into()
        }]
    );

    type_line(&mut room, ":ping");
    assert_eq!(room.drain(), vec![Command::Ping]);
    assert!(!room.is_typing());
}

#[test]
fn api_keys_are_typed_masked() {
    let mut room = room();
    room.drain();
    type_line(&mut room, ":key anthropic");
    assert!(room.is_typing());
    for ch in "sk-secret".chars() {
        room.key(Key::Char(ch));
    }
    let screen = text(&room.compose(140, 44));
    assert!(!screen.contains("sk-secret") && screen.contains("•••••••••"));
    room.key(Key::Enter);
    assert_eq!(
        room.drain(),
        vec![Command::SetApiKey {
            service: "anthropic".into(),
            key: Secret("sk-secret".into())
        }]
    );
}

#[test]
fn answers_go_to_the_runtime() {
    let mut room = room();
    room.drain();
    room.key(Key::Char('a'));
    assert!(matches!(
        &room.drain()[..],
        [Command::Answer {
            answer: Answer::Allow,
            ..
        }]
    ));
}

#[test]
fn acknowledging_quiets_the_caution() {
    let mut room = room();
    assert_eq!(room.alarm(), Alarm::Active);
    room.key(Key::Char('c'));
    assert_eq!(room.alarm(), Alarm::Acked);
}

#[test]
fn permission_modal_answers_and_defers() {
    let mut room = room();
    room.drain();
    let screen = text(&room.compose(140, 44));
    assert!(screen.contains("◆ PERMISSION REQUEST") && screen.contains("$ cargo test"));
    if std::env::var_os("SHOW").is_some() {
        println!("{screen}");
    }
    room.key(Key::Escape);
    let screen = text(&room.compose(140, 44));
    assert!(!screen.contains("◆ PERMISSION REQUEST"));
    // Still queued, still answerable.
    room.key(Key::Char('d'));
    assert!(matches!(
        &room.drain()[..],
        [Command::Answer {
            answer: Answer::Deny,
            ..
        }]
    ));
}

#[test]
fn risky_requests_show_their_warning() {
    let mut state = sample_state();
    state.permissions[0].warning = Some("writes outside the agent's worktree".into());
    let mut room = Room::new();
    room.set_state(state);
    let screen = text(&room.compose(140, 44));
    assert!(screen.contains("▲ writes outside the agent's worktree") && screen.contains("[G]roup"));
}

#[test]
fn palette_opens_and_runs_a_view() {
    let mut room = room();
    room.key(Key::Escape);
    room.drain();
    room.key(Key::Palette);
    for ch in "view log".chars() {
        room.key(Key::Char(ch));
    }
    room.key(Key::Enter);
    assert_eq!(room.view, View::Log);
    assert!(room.palette.is_none());
}

#[test]
fn long_log_lines_wrap_under_their_text() {
    let rows = wrap(
        "Cara, seguinte, eu preciso que você crie dois agentes para o projeto",
        24,
    );
    assert!(rows.len() > 1 && rows.iter().all(|r| r.chars().count() <= 24));
    assert_eq!(
        rows.join(" "),
        "Cara, seguinte, eu preciso que você crie dois agentes para o projeto"
    );

    let mut state = sample_state();
    let long = "heard: ".to_owned() + &"palavra ".repeat(60) + "FIM";
    state.log.push(LogEntry {
        at: chrono::Utc::now(),
        agent_id: None,
        level: LogLevel::Operator,
        text: long,
    });
    let mut room = Room::new();
    room.set_state(state);
    room.key(Key::Escape);
    room.key(Key::Function(4));
    let screen = text(&room.compose(120, 40));
    assert!(
        screen.contains("FIM"),
        "the end of a long line must stay visible"
    );
}

#[test]
fn empty_room_shows_the_guide() {
    let mut room = Room::new();
    room.set_state(AppState::default());
    let screen = text(&room.compose(140, 44));
    assert!(screen.contains("GETTING STARTED") && screen.contains(":open ~/code/my-repo"));
}

#[test]
fn open_projects_sit_left_of_the_sector_map() {
    let mut state = sample_state();
    let web = orch_core::model::Project {
        id: Uuid::new_v4(),
        name: "storefront".into(),
        root: "/tmp/storefront".into(),
    };
    let group = orch_core::model::Group {
        id: Uuid::new_v4(),
        project_id: web.id,
        name: "Web".into(),
        goal: "shop".into(),
        rules: Vec::new(),
        max_active: None,
    };
    let mut agent = state.agents[0].clone();
    agent.id = Uuid::new_v4();
    agent.group_id = group.id;
    agent.name = "Lojinha".into();
    state.projects.push(web.clone());
    state.groups.push(group);
    state.agents.push(agent);

    let mut room = Room::new();
    room.set_state(state.clone());
    let screen = text(&room.compose(180, 50));
    assert!(screen.contains("PROJECTS (2)"));
    assert!(screen.contains("storefront"));
    assert!(
        !screen.contains("Lojinha"),
        "only the focused project is on the map"
    );

    room.key(Key::Project(2));
    assert!(room.drain().contains(&Command::FocusProject(web.id)));
    state.focus = Some(web.id);
    room.set_state(state);
    room.key(Key::Escape); // the pending request (other project) waits
    let screen = text(&room.compose(180, 50));
    assert!(screen.contains("Lojinha"), "{screen}");
    assert!(screen.contains("ORCH//STOREFRONT"));
}
