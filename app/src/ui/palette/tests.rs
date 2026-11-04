use orch_core::{model::Provider, sample_data::sample_state};

use super::*;
use crate::ui::grid::Grid;

fn typed(palette: &mut Palette, text: &str) {
    for ch in text.chars() {
        palette.type_char(ch);
    }
}

#[test]
fn fuzzy_prefers_word_starts_and_runs() {
    assert!(fuzzy("na", "New agent").is_some());
    assert!(fuzzy("xyz", "New agent").is_none());
    let (word, _) = fuzzy("ng", "New group").unwrap();
    let (inner, _) = fuzzy("ng", "Opening").unwrap();
    assert!(word > inner);
}

#[test]
fn wizard_creates_an_agent_in_an_existing_group() {
    let mut state = sample_state();
    state.providers.push(orch_core::model::ProviderOption {
        label: "opencode".into(),
        provider: Provider::CliAcp {
            command: "opencode acp".into(),
        },
        ready: true,
    });
    let mut palette = Palette::new_agent();
    typed(&mut palette, "fro");
    assert_eq!(palette.enter(&state, None), Outcome::Keep);
    typed(&mut palette, "Reviewer");
    palette.enter(&state, None);
    typed(&mut palette, "open");
    palette.enter(&state, None);
    typed(&mut palette, "review the checkout");
    assert_eq!(palette.enter(&state, None), Outcome::Keep);
    typed(&mut palette, "picky about accessibility");
    assert_eq!(
        palette.enter(&state, None),
        Outcome::Done(Command::CreateAgent {
            group: "Front".into(),
            name: "Reviewer".into(),
            provider: Some("opencode".into()),
            task: "review the checkout".into(),
            persona: "picky about accessibility".into(),
        })
    );
}

#[test]
fn wizard_can_create_the_group_on_the_way_and_go_back() {
    let state = sample_state();
    let mut palette = Palette::new_agent();
    typed(&mut palette, "Docs");
    // The only match is "+ create group".
    assert_eq!(palette.enter(&state, None), Outcome::Keep);
    typed(&mut palette, "write the docs");
    assert_eq!(
        palette.enter(&state, None),
        Outcome::Send(Command::CreateGroup {
            name: "Docs".into(),
            goal: "write the docs".into()
        })
    );
    typed(&mut palette, "Writer");
    palette.enter(&state, None);
    // Esc goes back to the name with it filled in.
    palette.escape();
    assert_eq!(palette.query, "Writer");
}

#[test]
fn typed_commands_run_from_the_palette() {
    let state = sample_state();
    let mut palette = Palette::commands();
    typed(&mut palette, "open ~/code/shop");
    assert_eq!(
        palette.enter(&state, None),
        Outcome::Run("open ~/code/shop".into())
    );
}

#[test]
fn switching_provider_accepts_a_custom_model() {
    let mut state = sample_state();
    state.providers.push(orch_core::model::ProviderOption {
        label: "anthropic".into(),
        provider: Provider::Api {
            service: "anthropic".into(),
            model: "m".into(),
            key_ref: "k".into(),
        },
        ready: true,
    });
    let agent = state.agents[0].id;
    let mut palette = Palette::commands();
    typed(&mut palette, "switch provider");
    palette.enter(&state, Some(agent));
    typed(&mut palette, "anthropic/claude-opus-5-5");
    assert_eq!(
        palette.enter(&state, Some(agent)),
        Outcome::Done(Command::SetProvider {
            agent,
            provider: "anthropic/claude-opus-5-5".into()
        })
    );
}

#[test]
fn palette_draws_inside_the_grid() {
    let state = sample_state();
    let mut grid = Grid::new(140, 44);
    let rows = Palette::commands().draw(&mut grid, &state, None, true);
    assert!(!rows.is_empty());
    let text: String = (0..grid.rows)
        .map(|y| grid.row(y).iter().map(|c| c.ch).collect::<String>() + "\n")
        .collect();
    assert!(text.contains("⌘ COMMANDS") && text.contains("New agent"));
}

#[test]
fn open_project_lists_recent_and_found_repositories() {
    let mut state = sample_state();
    state.recent_projects = vec!["/home/me/www/shop".into()];
    state.repositories = vec!["/home/me/code/api".into(), "/home/me/www/shop".into()];
    let mut palette = Palette::open_project();
    typed(&mut palette, "api");
    assert_eq!(
        palette.enter(&state, None),
        Outcome::Done(Command::OpenProject {
            root: "/home/me/code/api".into()
        })
    );

    // A typed path is offered as is; Browse… asks for the folder chooser.
    let mut palette = Palette::open_project();
    typed(&mut palette, "/srv/repo");
    assert_eq!(
        palette.enter(&state, None),
        Outcome::Done(Command::OpenProject {
            root: "/srv/repo".into()
        })
    );
    let mut palette = Palette::open_project();
    typed(&mut palette, "browse");
    assert_eq!(palette.enter(&state, None), Outcome::Browse);
}

#[test]
fn mention_inserts_the_picked_file() {
    let state = sample_state();
    let mut palette = Palette::mention(vec!["src/payments/pix.rs".into(), "README.md".into()]);
    typed(&mut palette, "pix");
    assert_eq!(
        palette.enter(&state, None),
        Outcome::Insert("src/payments/pix.rs ".into())
    );
}

#[test]
fn personas_are_picked_or_typed_and_agents_need_no_project() {
    let mut state = sample_state();
    state.projects.clear();
    state.focus = None;
    state.providers.push(orch_core::model::ProviderOption {
        label: "opencode".into(),
        provider: Provider::CliAcp {
            command: "opencode acp".into(),
        },
        ready: true,
    });
    let mut palette = Palette::new_agent();
    // No project open: the personal workspace is offered first.
    palette.enter(&state, None);
    typed(&mut palette, "Mia");
    palette.enter(&state, None);
    palette.enter(&state, None);
    typed(&mut palette, "plan the launch campaign");
    palette.enter(&state, None);
    typed(&mut palette, "marketing");
    let Outcome::Done(Command::CreateAgent { group, persona, .. }) = palette.enter(&state, None)
    else {
        panic!("no agent created");
    };
    assert_eq!(group, "Personal");
    assert!(persona.starts_with("Marketing specialist."), "{persona}");

    // A short description of one's own is kept as typed.
    let state = sample_state();
    let agent = state.agents[0].id;
    let mut palette = Palette::commands();
    typed(&mut palette, "persona of");
    palette.enter(&state, Some(agent));
    typed(&mut palette, "SEO expert");
    assert_eq!(
        palette.enter(&state, Some(agent)),
        Outcome::Done(Command::SetPersona {
            agent,
            persona: "SEO expert".into()
        })
    );
}

#[test]
fn model_of_an_agent_lists_what_its_cli_offers() {
    let mut state = sample_state();
    let agent = state.agents[0].id;
    state.agents[0].models = vec!["gpt-6".into(), "gpt-6-astra".into()];
    state.agents[0].model = Some("gpt-6".into());
    let mut palette = Palette::commands();
    typed(&mut palette, "model of");
    palette.enter(&state, Some(agent));
    typed(&mut palette, "astra");
    assert_eq!(
        palette.enter(&state, Some(agent)),
        Outcome::Done(Command::SetModel {
            agent,
            model: "gpt-6-astra".into()
        })
    );
}

#[test]
fn color_of_an_agent_is_picked_from_the_palette() {
    let state = sample_state();
    let agent = state.agents[0].id;
    let mut palette = Palette::commands();
    typed(&mut palette, "color of");
    palette.enter(&state, Some(agent));
    typed(&mut palette, "pink");
    assert_eq!(
        palette.enter(&state, Some(agent)),
        Outcome::Done(Command::SetColor {
            agent,
            color: Some("pink".into())
        })
    );
}
