use super::*;
use crate::model::AgentState;
use crate::runner::fake::FakeRunner;

struct Room {
    commands: async_channel::Sender<Command>,
    events: async_channel::Receiver<Event>,
    repo: tempfile::TempDir,
    /// Holds the personal workspace of this test.
    home: tempfile::TempDir,
    /// Last snapshot seen, checked first: one snapshot may satisfy several waits.
    last: std::cell::RefCell<Option<AppState>>,
}

fn git_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"],
    ] {
        let status = std::process::Command::new("git").args(&args).current_dir(repo.path()).status().unwrap();
        assert!(status.success());
    }
    repo
}

fn room() -> Room {
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        store: None,
        voice: false,
        discover: false,
        permission_timeout: Duration::from_secs(600),
        runners: Arc::new(|_, context| Ok(Box::new(FakeRunner::new(context)) as Box<dyn AgentRunner>)),
        providers: ["fake", "fake2"]
            .into_iter()
            .map(|label| ProviderOption {
                label: label.into(),
                provider: Provider::CliAcp { command: label.into() },
                ready: true,
            })
            .collect(),
        sandbox: false,
        workspace: home.path().join("personal"),
        memory: home.path().join("memory"),
    };
    let (commands, rx) = async_channel::unbounded();
    let (tx, events) = async_channel::unbounded();
    tokio::spawn(run_with(config, rx, tx));
    Room { commands, events, repo: git_repo(), home, last: Default::default() }
}

impl Room {
    async fn send(&self, command: Command) {
        self.commands.send(command).await.unwrap();
    }

    /// Waits for a state snapshot that satisfies `check`.
    async fn until(&self, check: impl Fn(&AppState) -> bool) -> AppState {
        if let Some(state) = self.last.borrow().as_ref().filter(|s| check(s)) {
            return state.clone();
        }
        let wait = async {
            loop {
                if let Event::State(state) = self.events.recv().await.unwrap() {
                    let state = *state;
                    let matched = check(&state);
                    *self.last.borrow_mut() = Some(state.clone());
                    if matched {
                        return state;
                    }
                }
            }
        };
        match tokio::time::timeout(Duration::from_secs(10), wait).await {
            Ok(state) => state,
            Err(_) => panic!("state never matched; last log: {:#?}", self.last.borrow().as_ref().map(|s| &s.log)),
        }
    }

    /// Project with group API and agents Tests and Backend.
    async fn staffed(&self) -> AppState {
        self.send(Command::OpenProject { root: self.repo.path().into() }).await;
        self.send(Command::CreateGroup { name: "API".into(), goal: "Pix payments".into() }).await;
        for name in ["Tests", "Backend"] {
            self.send(Command::CreateAgent {
                group: "api".into(),
                name: name.into(),
                provider: Some("fake".into()),
                task: format!("{name} task"),
                persona: format!("{name} specialist"),
            })
            .await;
        }
        self.until(|s| s.agents.len() == 2).await
    }
}

fn agent_lines(state: &AppState, id: Uuid) -> Vec<String> {
    state.log.iter().filter(|e| e.agent_id == Some(id)).map(|e| e.text.clone()).collect()
}

#[tokio::test]
async fn publishes_state_and_answers_ping() {
    let room = room();
    room.until(|s| s.projects.is_empty()).await;
    room.send(Command::Ping).await;
    loop {
        if let Event::Pong { .. } = room.events.recv().await.unwrap() {
            break;
        }
    }
}

#[tokio::test]
async fn typed_text_routes_to_the_named_agent_in_its_own_worktree() {
    let room = room();
    let state = room.staffed().await;
    let (tests, backend) = (state.agents[0].id, state.agents[1].id);
    assert_ne!(state.agents[0].worktree, state.agents[1].worktree);

    room.send(Command::Say { text: "grupo API, agente Tests: rode a suíte".into(), selected: Some(backend) }).await;
    // A stopped agent starts with its task and the operator's words in one prompt.
    let state = room.until(|s| agent_lines(s, tests).iter().any(|l| l == "Operator: rode a suíte")).await;
    assert!(state.agents[0].worktree.join(".git").exists());
    assert!(agent_lines(&state, tests).contains(&"rode a suíte".to_owned()));
    assert!(agent_lines(&state, backend).is_empty());

    // Plain text goes to the selected agent.
    room.send(Command::Say { text: "add logs".into(), selected: Some(backend) }).await;
    room.until(|s| agent_lines(s, backend).iter().any(|l| l == "Operator: add logs")).await;
    room.until(|s| s.agents[1].state == AgentState::Done).await;
    // Once running, messages go straight through.
    room.send(Command::Say { text: "and metrics".into(), selected: Some(backend) }).await;
    room.until(|s| agent_lines(s, backend).iter().any(|l| l == "echo: and metrics")).await;
}

#[tokio::test]
async fn permission_waits_for_the_operator_and_resumes_the_agent() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents[0].id;
    room.send(Command::StartAgent(tests)).await;
    room.until(|s| s.agents[0].state == AgentState::Done).await;
    room.send(Command::Message { agent: tests, text: "perm:cargo test".into() }).await;
    let state = room.until(|s| !s.permissions.is_empty()).await;
    assert_eq!(state.agents[0].state, AgentState::AwaitingPermission);

    // Spoken "sempre" answers the open request and learns a rule.
    room.send(Command::Say { text: "Sempre.".into(), selected: None }).await;
    let state = room.until(|s| s.permissions.is_empty() && !s.agents[0].rules.is_empty()).await;
    assert_eq!(state.agents[0].rules[0].to_string(), "Bash(cargo test:*)");
    room.until(|s| agent_lines(s, tests).iter().any(|l| l == "cargo test: allowed")).await;

    // The same request again opens nothing.
    room.send(Command::Message { agent: tests, text: "perm:cargo test --lib".into() }).await;
    let state = room.until(|s| agent_lines(s, tests).iter().any(|l| l == "cargo test --lib: allowed")).await;
    assert!(state.permissions.is_empty());
}

#[tokio::test]
async fn multi_line_errors_become_one_log_line_each() {
    let room = room();
    room.send(Command::OpenProject { root: "/definitely/not/here".into() }).await;
    let state = room.until(|s| s.log.iter().any(|e| e.level == LogLevel::Error)).await;
    assert!(state.log.iter().all(|e| !e.text.contains('\n')));
}

#[tokio::test]
async fn switching_provider_mid_task_hands_the_conversation_over() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents[0].id;
    room.send(Command::Message { agent: tests, text: "remember the word pineapple".into() }).await;
    room.until(|s| s.agents[0].state == AgentState::Done).await;
    room.send(Command::SetProvider { agent: tests, provider: "fake2".into() }).await;
    let state = room
        .until(|s| {
            agent_lines(s, tests).iter().any(|l| l.contains("taking over this task from another model"))
                && agent_lines(s, tests).iter().any(|l| l.contains("Operator: remember the word pineapple"))
        })
        .await;
    assert_eq!(state.agents[0].provider, Provider::CliAcp { command: "fake2".into() });
}

#[tokio::test]
async fn group_limit_queues_extra_agents_until_a_slot_frees() {
    let room = room();
    let state = room.staffed().await;
    let (tests, backend) = (state.agents[0].id, state.agents[1].id);
    room.send(Command::SetGroupLimit { group: "API".into(), limit: Some(1) }).await;
    room.send(Command::StartAgent(tests)).await;
    room.until(|s| s.agents[0].state == AgentState::Done).await;
    room.send(Command::Message { agent: tests, text: "perm:cargo test".into() }).await;
    room.until(|s| s.agents[0].state == AgentState::AwaitingPermission).await;
    room.send(Command::StartAgent(backend)).await;
    room.until(|s| s.agents[1].state == AgentState::Queued).await;
    // Answering lets Tests finish its turn, which frees the slot.
    room.send(Command::Say { text: "permitir".into(), selected: None }).await;
    room.until(|s| s.agents[1].state != AgentState::Queued).await;
}

#[tokio::test]
async fn stop_marks_the_agent_stopped() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents[0].id;
    room.send(Command::StartAgent(tests)).await;
    room.until(|s| s.agents[0].state != AgentState::Stopped).await;
    room.send(Command::StopAgent(tests)).await;
    room.until(|s| s.agents[0].state == AgentState::Stopped && s.agents[0].started_at.is_none()).await;
}

#[tokio::test]
async fn projects_stay_open_side_by_side() {
    let room = room();
    let state = room.staffed().await;
    let first = state.focus.unwrap();
    let tests = state.agents.iter().find(|a| a.name == "Tests").unwrap().id;
    room.send(Command::StartAgent(tests)).await;
    room.until(|s| s.agent(tests).is_some_and(|a| a.state != AgentState::Stopped)).await;

    // A second project opens next to the first; the first keeps its agents running.
    let other = git_repo();
    room.send(Command::OpenProject { root: other.path().into() }).await;
    let state = room.until(|s| s.projects.len() == 2).await;
    let second = state.focus.unwrap();
    assert_ne!(first, second);
    assert_eq!(state.focused_groups().count(), 0);
    assert!(state.agent(tests).is_some_and(|a| a.state != AgentState::Stopped));

    // Groups are per project: the same name is fine in the second one.
    room.send(Command::CreateGroup { name: "API".into(), goal: "web".into() }).await;
    let state = room.until(|s| s.groups.len() == 2).await;
    assert_eq!(state.focused_groups().next().unwrap().project_id, second);

    // Reopening focuses instead of duplicating; selecting an agent focuses its project.
    room.send(Command::OpenProject { root: room.repo.path().into() }).await;
    room.until(|s| s.focus == Some(first) && s.projects.len() == 2).await;
    room.send(Command::FocusProject(second)).await;
    room.until(|s| s.focus == Some(second)).await;
    room.send(Command::Select(Some(tests))).await;
    room.until(|s| s.focus == Some(first)).await;

    // Closing stops and drops that project's agents only.
    room.send(Command::CloseProject(first)).await;
    let state = room.until(|s| s.projects.len() == 1).await;
    assert_eq!(state.focus, Some(second));
    assert!(state.agents.is_empty());
    assert_eq!(state.groups.len(), 1);
}

#[tokio::test]
async fn agents_ask_teammates_and_get_their_answer() {
    let room = room();
    let state = room.staffed().await;
    let id = |name: &str| state.agents.iter().find(|a| a.name == name).unwrap().id;
    let (tests, backend) = (id("Tests"), id("Backend"));

    // Backend is stopped: the question starts it, and its answer comes back to Tests.
    room.send(Command::Message { agent: tests, text: "ask:backend:what does /pix return?".into() }).await;
    let state = room.until(|s| agent_lines(s, tests).iter().any(|line| line.starts_with("backend said:"))).await;
    let answer = agent_lines(&state, tests).join("\n");
    assert!(answer.contains("Tests (group API) asks you: what does /pix return?"), "{answer}");
    assert!(answer.contains("You are Backend") && answer.contains("Backend specialist"), "{answer}");
    assert!(agent_lines(&state, backend).iter().any(|l| l.contains("← Tests asks")));

    // Unknown names and asking yourself fail fast, with the team listed.
    room.send(Command::Message { agent: tests, text: "ask:nobody:hi".into() }).await;
    room.until(|s| agent_lines(s, tests).iter().any(|l| l.contains("no teammate named nobody; teammates: Backend")))
        .await;
}

#[tokio::test]
async fn the_briefing_names_the_persona_and_the_team() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents.iter().find(|a| a.name == "Tests").unwrap().id;
    room.send(Command::SetPersona { agent: tests, persona: "only writes tests".into() }).await;
    room.until(|s| s.agent(tests).is_some_and(|a| a.persona == "only writes tests")).await;
    room.send(Command::StartAgent(tests)).await;
    let state = room.until(|s| agent_lines(s, tests).iter().any(|l| l.starts_with("echo:"))).await;
    let briefing = agent_lines(&state, tests).join("\n");
    assert!(briefing.contains("Who you are: only writes tests"), "{briefing}");
    assert!(briefing.contains("- Backend (API): Backend specialist"), "{briefing}");
    assert!(briefing.contains("Answer briefly"), "{briefing}");
    assert!(briefing.contains("ask_agent"), "{briefing}");
}

#[tokio::test]
async fn agents_need_no_project_open() {
    let room = room();
    room.until(|s| s.projects.is_empty()).await;
    room.send(Command::CreateAgent {
        group: "Personal".into(),
        name: "Mia".into(),
        provider: Some("fake".into()),
        task: "plan the launch campaign".into(),
        persona: "marketing specialist".into(),
    })
    .await;
    let state = room.until(|s| s.agents.len() == 1).await;
    let workspace = std::fs::canonicalize(room.home.path().join("personal")).unwrap();
    assert_eq!(state.project().map(|p| p.root.clone()), Some(workspace.clone()));
    assert_eq!(state.groups[0].name, "Personal");
    assert!(workspace.join(".git").exists());

    // The agent runs like any other, in a worktree of the workspace.
    let mia = state.agents[0].id;
    room.send(Command::Message { agent: mia, text: "hello".into() }).await;
    let state = room.until(|s| agent_lines(s, mia).iter().any(|l| l.starts_with("echo:"))).await;
    assert!(state.agents[0].worktree.starts_with(&workspace));
}

#[tokio::test]
async fn the_model_switches_within_the_provider() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents.iter().find(|a| a.name == "Tests").unwrap().id;
    room.send(Command::StartAgent(tests)).await;
    // The CLI reports what it offers.
    room.until(|s| s.agent(tests).is_some_and(|a| a.models == ["fake-1", "fake-2"])).await;
    room.send(Command::SetModel { agent: tests, model: "fake-2".into() }).await;
    let state = room.until(|s| s.agent(tests).is_some_and(|a| a.model.as_deref() == Some("fake-2"))).await;
    assert!(agent_lines(&state, tests).iter().any(|l| l.contains("model: fake-1 → fake-2")));

    // Another provider: the model and the list of the old one go away.
    room.send(Command::SetProvider { agent: tests, provider: "fake2".into() }).await;
    room.until(|s| s.agent(tests).is_some_and(|a| a.provider == Provider::CliAcp { command: "fake2".into() })).await;
    room.until(|s| {
        s.agent(tests).is_some_and(|a| a.models == ["fake-1", "fake-2"] && a.model.as_deref() == Some("fake-1"))
    })
    .await;
    room.send(Command::SetModel { agent: tests, model: "fake-2".into() }).await;
    room.until(|s| s.agent(tests).is_some_and(|a| a.model.as_deref() == Some("fake-2"))).await;

    // A model the CLI does not offer is refused.
    room.send(Command::SetModel { agent: tests, model: "gpt-9".into() }).await;
    room.until(|s| s.log.iter().any(|e| e.text.contains("does not offer gpt-9"))).await;
}

#[tokio::test]
async fn a_usage_limit_moves_the_agent_to_the_closest_model() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents.iter().find(|a| a.name == "Tests").unwrap().id;
    room.send(Command::Message { agent: tests, text: "write the refund tests".into() }).await;
    room.until(|s| agent_lines(s, tests).iter().any(|l| l.starts_with("echo:"))).await;

    room.send(Command::Message { agent: tests, text: "limit:".into() }).await;
    let state = room
        .until(|s| s.agent(tests).is_some_and(|a| a.provider == Provider::CliAcp { command: "fake2".into() }))
        .await;
    assert!(
        agent_lines(&state, tests).iter().any(|l| l.contains("limit reached on fake/fake-1 → going on with fake2"))
    );
    // It goes on by itself, with the conversation handed over.
    let state = room.until(|s| agent_lines(s, tests).iter().any(|l| l.contains("taking over this task"))).await;
    assert!(agent_lines(&state, tests).join("\n").contains("write the refund tests"));
}

#[tokio::test]
async fn a_model_the_cli_cannot_run_is_replaced_within_the_same_cli() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents.iter().find(|a| a.name == "Tests").unwrap().id;
    room.send(Command::StartAgent(tests)).await;
    room.until(|s| s.agent(tests).is_some_and(|a| a.models.len() == 2)).await;
    room.send(Command::Message { agent: tests, text: "nomodel:".into() }).await;
    let state = room
        .until(|s| {
            s.agent(tests).is_some_and(|a| {
                a.provider == Provider::CliAcp { command: "fake".into() } && a.model.as_deref() == Some("fake-2")
            })
        })
        .await;
    let lines = agent_lines(&state, tests);
    // The error reads as a sentence, and no update was attempted.
    assert!(lines.iter().any(|l| l == "The 'fake-1' model requires a newer version of Codex."), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("fake/fake-1 cannot run here → going on with fake/fake-2")));
    assert!(!lines.iter().any(|l| l.contains("updating")));
}

#[tokio::test]
async fn a_full_context_goes_on_with_a_summary_and_costs_add_up() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents.iter().find(|a| a.name == "Tests").unwrap().id;
    room.send(Command::Message { agent: tests, text: "refactor the handler".into() }).await;
    room.until(|s| agent_lines(s, tests).iter().any(|l| l.starts_with("echo:"))).await;
    room.send(Command::Message { agent: tests, text: "full:".into() }).await;
    // Past 85%: restarted with the conversation summarised, tool and app lines left out.
    let state =
        room.until(|s| agent_lines(s, tests).iter().any(|l| l.contains("earlier conversation was summarised"))).await;
    let lines = agent_lines(&state, tests);
    assert!(lines.iter().any(|l| l.contains("context 90% full → going on with a summary")));
    let summary = lines.join("\n");
    assert!(summary.contains("Operator: refactor the handler"), "{summary}");
    assert!(!summary.contains("Agent: starting with"), "{summary}");
    // The first run's cost stays counted after the restart.
    assert_eq!(state.agent(tests).unwrap().cost_micros, 250_000);

    room.send(Command::SetCompactAt(None)).await;
    room.until(|s| s.log.iter().any(|e| e.text.contains("keep their whole context"))).await;
}

#[tokio::test]
async fn folders_without_git_are_worked_in_directly() {
    let room = room();
    let plain = tempfile::tempdir().unwrap();
    room.send(Command::OpenProject { root: plain.path().into() }).await;
    let state = room.until(|s| s.projects.len() == 1).await;
    assert!(state.log.iter().any(|e| e.text.contains("agents work in the folder itself")));
    let root = state.projects[0].root.clone();
    room.send(Command::CreateGroup { name: "Site".into(), goal: String::new() }).await;
    room.send(Command::CreateAgent {
        group: "Site".into(),
        name: "Kio".into(),
        provider: Some("fake".into()),
        task: "fix the footer".into(),
        persona: String::new(),
    })
    .await;
    let state = room.until(|s| s.agents.len() == 1).await;
    let kio = state.agents[0].id;
    assert_eq!(state.agents[0].worktree, root);
    room.send(Command::Message { agent: kio, text: "go".into() }).await;
    let state = room.until(|s| agent_lines(s, kio).iter().any(|l| l.starts_with("echo:"))).await;
    assert!(!agent_lines(&state, kio).iter().any(|l| l.contains("not a git repository")));
    // Nothing to merge without git, and the reason is said.
    room.send(Command::Merge(kio)).await;
    room.until(|s| s.log.iter().any(|e| e.text.contains("nothing to merge"))).await;
}

#[tokio::test]
async fn a_global_agent_brings_its_memory_rules_and_persona_to_another_project() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents.iter().find(|a| a.name == "Tests").unwrap().id;
    // It learns something, then joins the library.
    room.send(Command::Message { agent: tests, text: "note:the operator prefers Pest".into() }).await;
    room.until(|s| agent_lines(s, tests).iter().any(|l| l.starts_with("noted"))).await;
    room.send(Command::SetPersona { agent: tests, persona: "tests only".into() }).await;
    room.send(Command::SaveProfile(tests)).await;
    let state = room.until(|s| s.profiles.len() == 1).await;
    let profile = state.profiles[0].clone();
    assert_eq!((profile.name.as_str(), profile.persona.as_str()), ("Tests", "tests only"));

    // In another project it is the same agent, with what it remembers.
    let other = git_repo();
    room.send(Command::OpenProject { root: other.path().into() }).await;
    room.until(|s| s.projects.len() == 2).await;
    room.send(Command::CreateGroup { name: "Web".into(), goal: String::new() }).await;
    room.send(Command::CreateFromProfile { profile: profile.id, group: "Web".into(), task: "test the shop".into() })
        .await;
    let state = room.until(|s| s.agents.iter().filter(|a| a.profile_id == Some(profile.id)).count() == 2).await;
    let twin = state.agents.iter().find(|a| a.profile_id == Some(profile.id) && a.id != tests).unwrap().id;
    room.send(Command::StartAgent(twin)).await;
    let state = room.until(|s| agent_lines(s, twin).iter().any(|l| l.starts_with("echo:"))).await;
    let briefing = agent_lines(&state, twin).join("\n");
    assert!(briefing.contains("Who you are: tests only"), "{briefing}");
    assert!(briefing.contains("the operator prefers Pest"), "{briefing}");

    // A change in one place reaches its twin and the library.
    room.send(Command::SetColor { agent: twin, color: Some("pink".into()) }).await;
    room.until(|s| s.agent(tests).is_some_and(|a| a.color.as_deref() == Some("pink"))).await;

    // Forgotten: the agents stay.
    room.send(Command::ForgetProfile("tests".into())).await;
    let state = room.until(|s| s.profiles.is_empty()).await;
    assert_eq!(state.agents.iter().filter(|a| a.profile_id == Some(profile.id)).count(), 2);
}

#[tokio::test]
async fn a_group_spans_other_projects_with_a_worktree_in_each() {
    let room = room();
    let state = room.staffed().await;
    let tests = state.agents.iter().find(|a| a.name == "Tests").unwrap().id;
    let site = git_repo();
    room.send(Command::AttachProject { group: "api".into(), root: site.path().into() }).await;
    let state = room.until(|s| s.groups.iter().any(|g| !g.attached.is_empty())).await;
    let root = state.groups[0].attached[0].clone();
    // Twice is once.
    room.send(Command::AttachProject { group: "api".into(), root: site.path().into() }).await;
    room.until(|s| s.log.iter().any(|e| e.text.contains("already works in"))).await;

    room.send(Command::Message { agent: tests, text: "go".into() }).await;
    let state = room.until(|s| agent_lines(s, tests).iter().any(|l| l.starts_with("echo:"))).await;
    let briefing = agent_lines(&state, tests).join("\n");
    assert!(briefing.contains("The group spans other projects too"), "{briefing}");
    let place = crate::worktree::path_for(&root, "Tests", tests);
    assert!(briefing.contains(&place.display().to_string()), "{briefing}");
    assert!(place.join(".git").exists(), "no worktree in the attached project");

    // Merging goes through every project.
    room.send(Command::Merge(tests)).await;
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    room.until(|s| s.log.iter().any(|e| e.text.contains(&format!("{name}: ")) && e.text.contains(" · "))).await;

    room.send(Command::DetachProject { group: "api".into(), project: name }).await;
    room.until(|s| s.groups[0].attached.is_empty()).await;
}

#[tokio::test]
async fn agents_without_a_name_get_a_free_one() {
    let room = room();
    room.staffed().await;
    for _ in 0..2 {
        room.send(Command::CreateAgent {
            group: "api".into(),
            name: String::new(),
            provider: Some("fake".into()),
            task: "review".into(),
            persona: String::new(),
        })
        .await;
    }
    let state = room.until(|s| s.agents.len() == 4).await;
    let names: Vec<&str> = state.agents.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(&names[2..], ["Atlas", "Iris"]);
}
