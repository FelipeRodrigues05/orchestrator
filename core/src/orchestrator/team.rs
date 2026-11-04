//! Agents of a project know and call each other (ADR 0018): the briefing, the
//! roster, `ask_agent` delivery and the turn bookkeeping that routes answers back.

use std::fmt::Write as _;
use uuid::Uuid;

use super::{AgentCmd, Orchestrator, Reply};
use crate::{
    model::{Agent, AgentState, Group, LogLevel, normalize},
    team::TeamRequest,
};

impl Orchestrator {
    /// What an agent is told when it starts: who it is, the goal, its task, its team.
    pub(super) fn briefing(&self, agent: &Agent, group: &Group) -> String {
        let project = self.state.project_of_group(group.id).map_or("the project", |p| p.name.as_str());
        let mut prompt = format!("You are {} (project {project}, group {}).", agent.name, group.name);
        if !agent.persona.is_empty() {
            let _ = write!(prompt, "\nWho you are: {}", agent.persona);
        }
        if !group.goal.is_empty() {
            let _ = write!(prompt, "\nGroup goal: {}", group.goal);
        }
        let _ = write!(prompt, "\nYour task: {}", agent.task);
        // Short answers save the most tokens: output costs more than input.
        prompt.push_str(
            "\nAnswer briefly: what you did, what is left, what you need. Do not restate the task or paste whole \
             files. @path/to/file names a file of the project.",
        );
        let team = self.roster(agent.id, true);
        if !team.is_empty() {
            let _ = write!(prompt, "\nTeammates (ask_agent to ask one, list_agents for details):\n{team}");
        }
        prompt
    }

    /// The other agents of `agent`'s project, one per line; empty when it works alone.
    /// `brief`: name, group and the first sentence of its role, for briefings; the
    /// `list_agents` tool gets everything.
    pub(super) fn roster(&self, agent: Uuid, brief: bool) -> String {
        let project = self.state.project_of_agent(agent).map(|p| p.id);
        self.state
            .agents_of(project)
            .filter(|a| a.id != agent)
            .map(|a| {
                let group = self.state.group(a.group_id).map_or("?", |g| g.name.as_str());
                let who = if a.persona.is_empty() { a.task.as_str() } else { a.persona.as_str() };
                if brief {
                    format!("- {} ({group}): {}", a.name, first_sentence(who, 80))
                } else {
                    format!("- {} (group {group}, {:?}): {who}", a.name, a.state)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(super) fn team_request(&mut self, request: TeamRequest) {
        match request {
            TeamRequest::Roster { from, reply } => {
                let roster = self.roster(from, false);
                let _ = reply.send(if roster.is_empty() {
                    "You have no teammates in this project.".into()
                } else {
                    roster
                });
            }
            TeamRequest::Ask { from, to, message, reply } => {
                if let Err(error) = self.ask(from, &to, &message, reply) {
                    self.log(Some(from), LogLevel::Warn, &format!("ask {to}: {error}"));
                }
            }
        }
    }

    /// Delivers a teammate's question; `reply` gets the answer when that turn ends.
    pub(super) fn ask(&mut self, from: Uuid, to: &str, message: &str, reply: Reply) -> Result<(), String> {
        let fail = |reply: Reply, error: String| {
            let _ = reply.send(Err(error.clone()));
            Err(error)
        };
        let Some(asker) = self.state.agent(from).cloned() else { return fail(reply, "unknown agent".into()) };
        let project = self.state.project_of_agent(from).map(|p| p.id);
        let wanted = normalize(to);
        let team: Vec<&Agent> = self.state.agents_of(project).filter(|a| a.id != from).collect();
        let target = team
            .iter()
            .find(|a| normalize(&a.name) == wanted)
            .or_else(|| team.iter().find(|a| !wanted.is_empty() && normalize(&a.name).starts_with(&wanted)))
            .map(|a| (a.id, a.name.clone()));
        let Some((target, target_name)) = target else {
            let names: Vec<&str> = team.iter().map(|a| a.name.as_str()).collect();
            let known = if names.is_empty() { "none".to_owned() } else { names.join(", ") };
            return fail(reply, format!("no teammate named {to}; teammates: {known}"));
        };
        // A waits for B, B asks A: neither turn could ever end.
        let mut waiting = Some(target);
        for _ in 0..16 {
            match waiting {
                Some(agent) if agent == from => {
                    return fail(
                        reply,
                        format!("{target_name} is waiting for your answer; answer it instead of asking back"),
                    );
                }
                Some(agent) => waiting = self.asking.get(&agent).copied(),
                None => break,
            }
        }
        let group = self.state.group(asker.group_id).map_or("?", |g| g.name.as_str());
        let text = format!(
            "{} (group {group}) asks you: {message}\n\nAnswer in your final message; it goes back to {}.",
            asker.name, asker.name
        );
        self.log(Some(from), LogLevel::Info, &format!("→ asks {target_name}: {message}"));
        self.log(Some(target), LogLevel::Info, &format!("← {} asks: {message}", asker.name));
        let turn = self.turns.get(&target).map_or(0, |book| book.sent) + 1;
        if self.running.contains_key(&target) {
            self.deliver(target, text);
        } else {
            let Some(agent) = self.state.agent(target).cloned() else { return Ok(()) };
            let Some(group) = self.state.group(agent.group_id).cloned() else { return Ok(()) };
            let prompt = format!("{}\n\n{text}", self.briefing(&agent, &group));
            self.launch(target, prompt);
        }
        self.asking.insert(from, target);
        self.turns.entry(target).or_default().waiters.push((turn, from, reply));
        Ok(())
    }

    /// Sends a message to a running agent: one more turn in its queue.
    pub(super) fn deliver(&mut self, id: Uuid, text: String) {
        let Some(agent) = self.running.get(&id) else { return };
        let _ = agent.send(AgentCmd::Send(text.clone()));
        self.turns.entry(id).or_default().sent += 1;
        self.last_prompt.insert(id, text);
        if let Some(agent) = self.agent_mut(id)
            && agent.state == AgentState::Done
        {
            agent.state = AgentState::Running;
        }
    }

    /// A turn ended: whoever asked for it gets its text.
    pub(super) fn turn_ended(&mut self, id: Uuid) {
        let book = self.turns.entry(id).or_default();
        book.ended += 1;
        let text = std::mem::take(&mut book.text);
        let ended = book.ended;
        let (done, pending): (Vec<_>, Vec<_>) = book.waiters.drain(..).partition(|(turn, ..)| *turn <= ended);
        book.waiters = pending;
        let answer = text.trim().to_owned();
        for (_, asker, reply) in done {
            self.asking.remove(&asker);
            let answer =
                if answer.is_empty() { "(it finished without a written answer)".to_owned() } else { answer.clone() };
            let _ = reply.send(Ok(answer));
        }
    }

    /// The agent stopped or failed: nobody will get an answer from it.
    pub(super) fn turns_lost(&mut self, id: Uuid, why: &str) {
        let name = self.state.agent(id).map_or("the agent".to_owned(), |a| a.name.clone());
        let Some(book) = self.turns.get_mut(&id) else { return };
        book.ended = book.sent;
        book.text.clear();
        for (_, asker, reply) in book.waiters.drain(..) {
            self.asking.remove(&asker);
            let _ = reply.send(Err(format!("{name} {why} before answering")));
        }
    }
}

/// The text up to its first full stop, at most `limit` characters.
fn first_sentence(text: &str, limit: usize) -> String {
    let sentence = text.split_once(". ").map_or(text, |(first, _)| first).trim_end_matches('.');
    if sentence.chars().count() <= limit {
        return sentence.to_owned();
    }
    let cut: String = sentence.chars().take(limit).collect();
    format!("{}…", cut.trim_end())
}
