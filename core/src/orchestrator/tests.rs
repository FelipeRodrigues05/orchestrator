use super::*;
use crate::model::AgentState;
use crate::runner::fake::FakeRunner;

struct Room {
    commands: async_channel::Sender<Command>,
    events: async_channel::Receiver<Event>,
    repo: tempfile::TempDir,
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
    };
    let (commands, rx) = async_channel::unbounded();
    let (tx, events) = async_channel::unbounded();
    tokio::spawn(run_with(config, rx, tx));
    Room { commands, events, repo: git_repo(), last: Default::default() }
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
    assert!(briefing.contains("- Backend (group API, Stopped): Backend specialist"), "{briefing}");
    assert!(briefing.contains("ask_agent"), "{briefing}");
}
