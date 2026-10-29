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
