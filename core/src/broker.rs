//! Permission broker: the single place where sensitive actions are allowed or denied
//! (ADR 0005). Checks the agent's rules, then the group's, then safe defaults; anything
//! left is queued for the operator (UI or voice) and expires into a denial.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{
    model::{Decision, Effect, PermissionRequest, PermissionRule, resolve},
    voice::{Priority, VoiceHandle, phrases::Phrase},
};

/// What the operator answered, before it becomes a `Decision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Allow,
    /// Allow and remember for this agent.
    AlwaysAllow,
    /// Allow and remember for the agent's whole group.
    AlwaysAllowGroup,
    Deny,
}

/// Where a decision came from, for the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionSource {
    Rule,
    Default,
    Click,
    Voice,
    Timeout,
    Cancelled,
}

impl DecisionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionSource::Rule => "rule",
            DecisionSource::Default => "default",
            DecisionSource::Click => "click",
            DecisionSource::Voice => "voice",
            DecisionSource::Timeout => "timeout",
            DecisionSource::Cancelled => "cancelled",
        }
    }
}

/// What the broker needs besides the request to decide and announce it.
#[derive(Debug, Clone)]
pub struct AskContext {
    pub group_id: Uuid,
    pub agent_name: String,
    pub group_name: String,
    pub worktree: PathBuf,
    /// The agent's places in the group's other projects (ADR 0024): inside, too.
    pub extra: Vec<PathBuf>,
    /// Files the tool touches, absolute or relative to the worktree.
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum BrokerEvent {
    /// Waiting for the operator.
    Queued(PermissionRequest),
    Resolved {
        request: PermissionRequest,
        decision: Decision,
        source: DecisionSource,
    },
    /// "Always allow" added this rule to an agent or a group.
    RuleLearned {
        scope: RuleScope,
        rule: PermissionRule,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleScope {
    Agent(Uuid),
    Group(Uuid),
}

enum Msg {
    Ask { request: Box<PermissionRequest>, context: Box<AskContext>, reply: oneshot::Sender<Decision> },
    Answer { request: Option<Uuid>, answer: Answer, source: DecisionSource },
    SetRules { scope: RuleScope, rules: Vec<PermissionRule> },
    SetTimeout(Duration),
    Finished { id: Uuid, decision: Decision, source: DecisionSource },
}

#[derive(Clone)]
pub struct BrokerHandle {
    tx: mpsc::UnboundedSender<Msg>,
}

impl BrokerHandle {
    /// Waits for a decision. If the broker is gone, the answer is a denial.
    pub async fn ask(&self, request: PermissionRequest, context: AskContext) -> Decision {
        let (reply, decision) = oneshot::channel();
        if self.tx.send(Msg::Ask { request: Box::new(request), context: Box::new(context), reply }).is_err() {
            return Decision::Deny { reason: "permission broker stopped".into() };
        }
        decision.await.unwrap_or(Decision::Deny { reason: "permission broker stopped".into() })
    }

    /// Answers `request`, or the oldest pending one when `None` (voice answers).
    pub fn answer(&self, request: Option<Uuid>, answer: Answer, source: DecisionSource) {
        let _ = self.tx.send(Msg::Answer { request, answer, source });
    }

    pub fn set_rules(&self, scope: RuleScope, rules: Vec<PermissionRule>) {
        let _ = self.tx.send(Msg::SetRules { scope, rules });
    }

    /// Applies to requests that arrive from now on.
    pub fn set_timeout(&self, timeout: Duration) {
        let _ = self.tx.send(Msg::SetTimeout(timeout));
    }
}

struct Pending {
    request: PermissionRequest,
    group_id: Uuid,
    /// The "always" answer was for the whole group.
    to_group: bool,
    /// `None` once answered, while the waiter reports back.
    decide: Option<oneshot::Sender<(Decision, DecisionSource)>>,
}

struct Broker {
    timeout: Duration,
    tx: mpsc::UnboundedSender<Msg>,
    events: mpsc::UnboundedSender<BrokerEvent>,
    voice: Option<VoiceHandle>,
    agent_rules: HashMap<Uuid, Vec<PermissionRule>>,
    group_rules: HashMap<Uuid, Vec<PermissionRule>>,
    /// Oldest first.
    queue: Vec<Pending>,
}

pub fn spawn(
    timeout: Duration,
    events: mpsc::UnboundedSender<BrokerEvent>,
    voice: Option<VoiceHandle>,
) -> BrokerHandle {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut broker = Broker {
        timeout,
        tx: tx.clone(),
        events,
        voice,
        agent_rules: HashMap::new(),
        group_rules: HashMap::new(),
        queue: Vec::new(),
    };
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            broker.handle(msg);
        }
    });
    BrokerHandle { tx }
}

impl Broker {
    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Ask { request, context, reply } => self.ask(*request, &context, reply),
            Msg::Answer { request, answer, source } => {
                // The entry stays queued until its waiter reports `Finished`.
                let pending =
                    self.queue.iter_mut().find(|p| p.decide.is_some() && request.is_none_or(|id| p.request.id == id));
                if let Some(pending) = pending {
                    let decision = match answer {
                        Answer::Allow => Decision::Allow,
                        // Risky requests are never turned into rules.
                        Answer::AlwaysAllow | Answer::AlwaysAllowGroup if pending.request.warning.is_some() => {
                            Decision::Allow
                        }
                        Answer::AlwaysAllow | Answer::AlwaysAllowGroup => {
                            pending.to_group = answer == Answer::AlwaysAllowGroup;
                            Decision::AlwaysAllow(PermissionRule::suggest(&pending.request))
                        }
                        Answer::Deny => Decision::Deny { reason: "denied by the operator".into() },
                    };
                    if let Some(decide) = pending.decide.take() {
                        let _ = decide.send((decision, source));
                    }
                }
            }
            Msg::SetTimeout(timeout) => self.timeout = timeout,
            Msg::SetRules { scope, rules } => match scope {
                RuleScope::Agent(id) => {
                    self.agent_rules.insert(id, rules);
                }
                RuleScope::Group(id) => {
                    self.group_rules.insert(id, rules);
                }
            },
            Msg::Finished { id, decision, source } => {
                let Some(index) = self.queue.iter().position(|p| p.request.id == id) else {
                    return;
                };
                let pending = self.queue.remove(index);
                if let Decision::AlwaysAllow(rule) = &decision {
                    let scope = if pending.to_group {
                        self.group_rules.entry(pending.group_id).or_default().push(rule.clone());
                        RuleScope::Group(pending.group_id)
                    } else {
                        self.agent_rules.entry(pending.request.agent_id).or_default().push(rule.clone());
                        RuleScope::Agent(pending.request.agent_id)
                    };
                    let _ = self.events.send(BrokerEvent::RuleLearned { scope, rule: rule.clone() });
                }
                let _ = self.events.send(BrokerEvent::Resolved { request: pending.request, decision, source });
            }
        }
    }

    fn ask(&mut self, mut request: PermissionRequest, context: &AskContext, reply: oneshot::Sender<Decision>) {
        request.timeout_secs = self.timeout.as_secs() as i64;
        // Commands writing outside the worktree always reach the operator (ADR 0006).
        let risky = request.tool.eq_ignore_ascii_case("bash")
            && writes_outside(&request.input_summary(), &context.worktree, &context.extra);
        if risky {
            request.warning = Some("writes outside the agent's worktree".into());
        }
        let automatic = if risky { None } else { self.decide_without_asking(&request, context) };
        if let Some((decision, source)) = automatic {
            let _ = reply.send(decision.clone());
            let _ = self.events.send(BrokerEvent::Resolved { request, decision, source });
            return;
        }

        let (decide, decided) = oneshot::channel();
        let id = request.id;
        let finished = self.tx.clone();
        let timeout = self.timeout;
        tokio::spawn(async move {
            let mut reply = reply;
            let (decision, source) = tokio::select! {
                decided = decided => match decided {
                    Ok(outcome) => outcome,
                    Err(_) => return,
                },
                () = tokio::time::sleep(timeout) => {
                    (Decision::Deny { reason: "no response".into() }, DecisionSource::Timeout)
                }
                () = reply.closed() => {
                    (Decision::Deny { reason: "the agent stopped waiting".into() }, DecisionSource::Cancelled)
                }
            };
            let _ = reply.send(decision.clone());
            let _ = finished.send(Msg::Finished { id, decision, source });
        });

        if let Some(voice) = &self.voice {
            voice.announce(
                Priority::Permission,
                Phrase::Permission {
                    agent: context.agent_name.clone(),
                    group: context.group_name.clone(),
                    tool: request.tool.clone(),
                    what: request.input_summary(),
                },
            );
        }
        let _ = self.events.send(BrokerEvent::Queued(request.clone()));
        self.queue.push(Pending { request, group_id: context.group_id, to_group: false, decide: Some(decide) });
    }

    fn decide_without_asking(
        &self,
        request: &PermissionRequest,
        context: &AskContext,
    ) -> Option<(Decision, DecisionSource)> {
        let summary = request.input_summary();
        let empty = Vec::new();
        let rules = [
            self.agent_rules.get(&request.agent_id).unwrap_or(&empty),
            self.group_rules.get(&context.group_id).unwrap_or(&empty),
        ];
        for list in rules {
            if let Some(rule) = list.iter().find(|r| r.matches(&request.tool, &summary)) {
                return match rule.effect {
                    Effect::Allow => Some((Decision::Allow, DecisionSource::Rule)),
                    Effect::Deny => Some((Decision::Deny { reason: format!("rule {rule}") }, DecisionSource::Rule)),
                    Effect::Ask => None,
                };
            }
        }
        safe_default(&request.tool, context)
    }
}

/// Reads stay allowed inside the worktree; anything touching files outside it is
/// denied (ADR 0006).
fn safe_default(tool: &str, context: &AskContext) -> Option<(Decision, DecisionSource)> {
    // Talking to a teammate touches nothing; the teammate's own actions are asked for.
    if crate::team::is_team_tool(tool) {
        return Some((Decision::Allow, DecisionSource::Default));
    }
    let outside = context.paths.iter().any(|path| !inside(&context.worktree, &context.extra, path));
    let tool = tool.to_ascii_lowercase();
    let reads = matches!(tool.as_str(), "read" | "search" | "think");
    let writes = matches!(tool.as_str(), "edit" | "write" | "delete" | "move");
    if outside && (writes || reads) {
        let reason = "outside the agent's worktree".to_owned();
        return Some((Decision::Deny { reason }, DecisionSource::Default));
    }
    reads.then_some((Decision::Allow, DecisionSource::Default))
}

/// Whether a shell command looks like it writes to a path outside `worktree` (and
/// the `extra` folders). A heuristic until the sandbox (step 9) enforces it.
pub fn writes_outside(command: &str, worktree: &Path, extra: &[PathBuf]) -> bool {
    static WRITES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(^|[;&|(]\s*|\s)(rm|rmdir|mv|cp|tee|touch|mkdir|chmod|chown|ln|dd|install|truncate|rsync)\b|\bsed\s+-[a-zA-Z]*i|>",
        )
        .expect("valid regex")
    });
    if !WRITES.is_match(command) {
        return false;
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '"' | '\''))
        .map(|token| token.trim_start_matches(['>', '<', '=']))
        .filter(|token| token.starts_with('/') || token.starts_with('~') || token.contains(".."))
        .filter(|token| !matches!(*token, "/dev/null" | "/dev/stdout" | "/dev/stderr"))
        .any(|token| {
            let path = match token.strip_prefix('~') {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => std::path::PathBuf::from(token),
            };
            !inside(worktree, extra, &path)
        })
}

/// Whether `path` (relative to the worktree) lies in the worktree or an extra folder.
fn inside(worktree: &Path, extra: &[PathBuf], path: &Path) -> bool {
    let full = resolve(worktree, path);
    full.starts_with(resolve(worktree, Path::new("."))) || extra.iter().any(|folder| full.starts_with(folder))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;

    struct Setup {
        broker: BrokerHandle,
        events: mpsc::UnboundedReceiver<BrokerEvent>,
        agent: Uuid,
        group: Uuid,
    }

    fn setup() -> Setup {
        let (tx, events) = mpsc::unbounded_channel();
        let broker = spawn(Duration::from_secs(600), tx, None);
        Setup { broker, events, agent: Uuid::new_v4(), group: Uuid::new_v4() }
    }

    fn request(agent: Uuid, tool: &str, command: &str) -> PermissionRequest {
        PermissionRequest {
            id: Uuid::new_v4(),
            agent_id: agent,
            tool: tool.into(),
            input: json!({ "command": command }),
            spoken_summary: String::new(),
            created_at: Utc::now(),
            timeout_secs: 600,
            warning: None,
        }
    }

    fn context(group: Uuid) -> AskContext {
        AskContext {
            group_id: group,
            agent_name: "Tests".into(),
            group_name: "API".into(),
            worktree: "/tmp/shop/.worktrees/tests".into(),
            extra: Vec::new(),
            paths: Vec::new(),
        }
    }

    fn rule(text: &str, effect: Effect) -> PermissionRule {
        PermissionRule::parse(text, effect).unwrap()
    }

    async fn queued(events: &mut mpsc::UnboundedReceiver<BrokerEvent>) -> PermissionRequest {
        match events.recv().await.unwrap() {
            BrokerEvent::Queued(request) => request,
            other => panic!("expected Queued, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_rule_answers_without_ui() {
        let mut s = setup();
        s.broker.set_rules(RuleScope::Agent(s.agent), vec![rule("Bash(cargo test:*)", Effect::Allow)]);
        let decision = s.broker.ask(request(s.agent, "Bash", "cargo test --lib"), context(s.group)).await;
        assert_eq!(decision, Decision::Allow);
        assert!(matches!(s.events.recv().await.unwrap(), BrokerEvent::Resolved { source: DecisionSource::Rule, .. }));
    }

    #[tokio::test]
    async fn group_rule_applies_when_the_agent_has_none() {
        let s = setup();
        s.broker.set_rules(RuleScope::Group(s.group), vec![rule("Bash(rm:*)", Effect::Deny)]);
        let decision = s.broker.ask(request(s.agent, "Bash", "rm -rf target"), context(s.group)).await;
        assert!(matches!(decision, Decision::Deny { .. }));
    }

    #[tokio::test]
    async fn no_rule_queues_and_the_answer_reaches_the_agent() {
        let mut s = setup();
        let broker = s.broker.clone();
        let (agent, group) = (s.agent, s.group);
        let waiting =
            tokio::spawn(async move { broker.ask(request(agent, "Bash", "cargo build"), context(group)).await });
        let queued = queued(&mut s.events).await;
        s.broker.answer(Some(queued.id), Answer::Allow, DecisionSource::Click);
        assert_eq!(waiting.await.unwrap(), Decision::Allow);
        assert!(matches!(s.events.recv().await.unwrap(), BrokerEvent::Resolved { source: DecisionSource::Click, .. }));
    }

    #[tokio::test]
    async fn simultaneous_requests_queue_in_order() {
        let mut s = setup();
        let mut waiters = Vec::new();
        for command in ["first", "second"] {
            let broker = s.broker.clone();
            let (agent, group) = (s.agent, s.group);
            waiters
                .push(tokio::spawn(async move { broker.ask(request(agent, "Bash", command), context(group)).await }));
            queued(&mut s.events).await;
        }
        // Voice answers the oldest one.
        s.broker.answer(None, Answer::Deny, DecisionSource::Voice);
        let BrokerEvent::Resolved { request, .. } = s.events.recv().await.unwrap() else { panic!() };
        assert_eq!(request.input_summary(), "first");
        s.broker.answer(None, Answer::Allow, DecisionSource::Voice);
        let second = waiters.pop().unwrap().await.unwrap();
        let first = waiters.pop().unwrap().await.unwrap();
        assert!(matches!(first, Decision::Deny { .. }));
        assert_eq!(second, Decision::Allow);
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_requests_expire_into_a_denial() {
        let (tx, mut events) = mpsc::unbounded_channel();
        let broker = spawn(Duration::from_secs(600), tx, None);
        let (agent, group) = (Uuid::new_v4(), Uuid::new_v4());
        let waiting = tokio::spawn({
            let broker = broker.clone();
            async move { broker.ask(request(agent, "Bash", "cargo build"), context(group)).await }
        });
        queued(&mut events).await;
        tokio::time::advance(Duration::from_secs(601)).await;
        assert_eq!(waiting.await.unwrap(), Decision::Deny { reason: "no response".into() });
    }

    #[tokio::test]
    async fn always_allow_covers_the_next_identical_request() {
        let mut s = setup();
        let broker = s.broker.clone();
        let (agent, group) = (s.agent, s.group);
        let waiting =
            tokio::spawn(async move { broker.ask(request(agent, "Bash", "cargo test"), context(group)).await });
        let queued = queued(&mut s.events).await;
        s.broker.answer(Some(queued.id), Answer::AlwaysAllow, DecisionSource::Click);
        assert!(matches!(waiting.await.unwrap(), Decision::AlwaysAllow(_)));

        let again = s.broker.ask(request(s.agent, "Bash", "cargo test -- pix"), context(s.group)).await;
        assert_eq!(again, Decision::Allow);
    }

    #[tokio::test]
    async fn writes_outside_the_worktree_are_denied_and_reads_inside_allowed() {
        let s = setup();
        let mut outside = context(s.group);
        outside.paths = vec!["../../etc/passwd".into()];
        let decision = s.broker.ask(request(s.agent, "Edit", ""), outside).await;
        assert!(matches!(decision, Decision::Deny { .. }));

        let mut inside = context(s.group);
        inside.paths = vec!["src/lib.rs".into()];
        assert_eq!(s.broker.ask(request(s.agent, "Read", ""), inside).await, Decision::Allow);
    }

    #[test]
    fn spots_shell_writes_outside_the_worktree() {
        let worktree = Path::new("/tmp/shop/.worktrees/tests");
        assert!(writes_outside("rm -rf /home/me/project", worktree, &[]));
        assert!(writes_outside("echo hi > ../../outside.txt", worktree, &[]));
        assert!(writes_outside("cp a.txt ~/notes.txt", worktree, &[]));
        assert!(!writes_outside("rm -rf target", worktree, &[]));
        assert!(!writes_outside("cat /etc/hosts", worktree, &[]));
        assert!(!writes_outside("cargo test 2>/dev/null", worktree, &[]));
        assert!(!writes_outside("echo ok > /tmp/shop/.worktrees/tests/out.txt", worktree, &[]));
        // The group's other projects count as inside (ADR 0024).
        let extra = [PathBuf::from("/tmp/site/.worktrees/tests")];
        assert!(!writes_outside("echo ok > /tmp/site/.worktrees/tests/a.txt", worktree, &extra));
        assert!(writes_outside("echo ok > /tmp/site/a.txt", worktree, &extra));
    }

    #[tokio::test]
    async fn risky_commands_ignore_rules_and_never_become_rules() {
        let mut s = setup();
        s.broker.set_rules(RuleScope::Agent(s.agent), vec![rule("Bash(rm:*)", Effect::Allow)]);
        let broker = s.broker.clone();
        let (agent, group) = (s.agent, s.group);
        let waiting =
            tokio::spawn(async move { broker.ask(request(agent, "Bash", "rm -rf /home/x"), context(group)).await });
        let queued = queued(&mut s.events).await;
        assert!(queued.warning.is_some());
        s.broker.answer(Some(queued.id), Answer::AlwaysAllow, DecisionSource::Click);
        assert_eq!(waiting.await.unwrap(), Decision::Allow);
    }

    #[tokio::test]
    async fn always_allow_for_the_group_covers_its_other_agents() {
        let mut s = setup();
        let broker = s.broker.clone();
        let (agent, group) = (s.agent, s.group);
        let waiting =
            tokio::spawn(async move { broker.ask(request(agent, "Bash", "cargo fmt"), context(group)).await });
        let queued = queued(&mut s.events).await;
        s.broker.answer(Some(queued.id), Answer::AlwaysAllowGroup, DecisionSource::Click);
        waiting.await.unwrap();
        let sibling = Uuid::new_v4();
        assert_eq!(
            s.broker.ask(request(sibling, "Bash", "cargo fmt --check"), context(s.group)).await,
            Decision::Allow
        );
    }

    #[tokio::test]
    async fn a_stopped_agent_drops_its_request() {
        let mut s = setup();
        let broker = s.broker.clone();
        let (agent, group) = (s.agent, s.group);
        let waiting = tokio::spawn(async move { broker.ask(request(agent, "Bash", "sleep"), context(group)).await });
        queued(&mut s.events).await;
        waiting.abort();
        assert!(matches!(
            s.events.recv().await.unwrap(),
            BrokerEvent::Resolved { source: DecisionSource::Cancelled, .. }
        ));
    }
}
