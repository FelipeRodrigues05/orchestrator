//! The agent library (ADR 0023): global agents used in any project, kept in step
//! across the projects they work in, and the memory every agent keeps.

use uuid::Uuid;

use super::{Orchestrator, Reply};
use crate::{
    broker::RuleScope,
    memory,
    model::{Agent, LogLevel, PermissionRule, Profile, normalize},
};

impl Orchestrator {
    /// Whose memory an agent reads and writes: its global agent's, or its own.
    pub(super) fn memory_key(agent: &Agent) -> Uuid {
        agent.profile_id.unwrap_or(agent.id)
    }

    /// The latest notes the agent's memory holds, for its briefing.
    pub(super) fn memory_of(&self, agent: &Agent) -> String {
        memory::recent(&self.memory_dir, Self::memory_key(agent), memory::BRIEFING_LIMIT)
    }

    /// A note for the agent's memory, from itself (the `remember` tool) or the operator.
    pub(super) fn remember(&mut self, id: Uuid, note: &str, reply: Option<Reply>) {
        let Some(agent) = self.state.agent(id).cloned() else { return };
        let result = memory::append(&self.memory_dir, Self::memory_key(&agent), note).map_err(|e| e.to_string());
        match &result {
            Ok(()) => self.log(Some(id), LogLevel::Info, &format!("remembers: {}", note.trim())),
            Err(error) => self.log(Some(id), LogLevel::Warn, &format!("could not remember: {error}")),
        }
        if let Some(reply) = reply {
            let _ = reply.send(result.map(|()| "noted".to_owned()));
        }
    }

    /// Puts the agent in the library: it becomes a global agent, usable in any
    /// project, and keeps what it remembers.
    pub(super) fn save_profile(&mut self, id: Uuid) {
        let Some(agent) = self.state.agent(id).cloned() else { return };
        if let Some(profile) = agent.profile_id.and_then(|p| self.state.profiles.iter().find(|x| x.id == p)) {
            self.notice(LogLevel::Info, format!("{} is already in the library", profile.name));
            return;
        }
        if self.state.profiles.iter().any(|p| normalize(&p.name) == normalize(&agent.name)) {
            self.notice(LogLevel::Warn, format!("the library already has an agent named {}", agent.name));
            return;
        }
        let profile = Profile {
            id: Uuid::new_v4(),
            name: agent.name.clone(),
            persona: agent.persona.clone(),
            provider: agent.provider.clone(),
            model: agent.model.clone(),
            color: agent.color.clone(),
            rules: agent.rules.clone(),
        };
        if let Err(error) = memory::rename(&self.memory_dir, agent.id, profile.id) {
            self.notice(LogLevel::Warn, format!("{}'s memory stayed behind: {error}", agent.name));
        }
        if let Some(store) = &self.store {
            store.save_profile(&profile);
        }
        if let Some(target) = self.agent_mut(id) {
            target.profile_id = Some(profile.id);
            let snapshot = target.clone();
            if let Some(store) = &self.store {
                store.save_agent(&snapshot);
            }
        }
        self.notice(LogLevel::Info, format!("{} is in the library: use it in any project with n", profile.name));
        self.state.profiles.push(profile);
        self.state.profiles.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// The library's agent at work in `group` of the focused project, on `task`.
    pub(super) fn create_from_profile(&mut self, profile: Uuid, group: &str, task: &str) {
        let Some(profile) = self.state.profiles.iter().find(|p| p.id == profile).cloned() else {
            self.notice(LogLevel::Warn, "that agent is no longer in the library");
            return;
        };
        let Some(id) = self.create_agent(group, &profile.name, None, task, &profile.persona) else { return };
        if let Some(agent) = self.agent_mut(id) {
            agent.provider = profile.provider.clone();
            agent.model.clone_from(&profile.model);
            agent.color.clone_from(&profile.color);
            agent.rules.clone_from(&profile.rules);
            agent.profile_id = Some(profile.id);
            let snapshot = agent.clone();
            if let Some(store) = &self.store {
                store.save_agent(&snapshot);
            }
        }
        self.broker.set_rules(RuleScope::Agent(id), profile.rules);
    }

    /// Takes a global agent out of the library. Agents already made from it keep
    /// working, and keep its memory, but no longer follow each other.
    pub(super) fn forget_profile(&mut self, name: &str) {
        let wanted = normalize(name);
        let Some(index) = self.state.profiles.iter().position(|p| normalize(&p.name) == wanted) else {
            self.notice(LogLevel::Warn, format!("the library has no agent named {name}"));
            return;
        };
        let profile = self.state.profiles.remove(index);
        if let Some(store) = &self.store {
            store.delete_profile(profile.id);
        }
        self.notice(LogLevel::Info, format!("{} left the library; its agents in projects stay", profile.name));
    }

    /// After the operator changes a global agent somewhere (persona, model, colour,
    /// provider), the library and its other agents follow.
    pub(super) fn sync_profile(&mut self, id: Uuid) {
        let Some(agent) = self.state.agent(id).cloned() else { return };
        let Some(profile) = agent.profile_id.and_then(|p| self.state.profiles.iter_mut().find(|x| x.id == p)) else {
            return;
        };
        profile.persona.clone_from(&agent.persona);
        profile.provider = agent.provider.clone();
        profile.model.clone_from(&agent.model);
        profile.color.clone_from(&agent.color);
        let snapshot = profile.clone();
        if let Some(store) = &self.store {
            store.save_profile(&snapshot);
        }
        for other in self.state.agents.iter_mut().filter(|a| a.profile_id == Some(snapshot.id) && a.id != id) {
            other.persona.clone_from(&snapshot.persona);
            other.provider = snapshot.provider.clone();
            other.model.clone_from(&snapshot.model);
            other.color.clone_from(&snapshot.color);
            if let Some(store) = &self.store {
                store.save_agent(other);
            }
        }
    }

    /// A rule learnt by a global agent holds in every project it works in.
    pub(super) fn share_rule(&mut self, id: Uuid, rule: &PermissionRule) {
        let Some(profile_id) = self.state.agent(id).and_then(|a| a.profile_id) else { return };
        let Some(profile) = self.state.profiles.iter_mut().find(|p| p.id == profile_id) else { return };
        if !profile.rules.contains(rule) {
            profile.rules.push(rule.clone());
            if let Some(store) = &self.store {
                store.save_profile(profile);
            }
        }
        for other in self.state.agents.iter_mut().filter(|a| a.profile_id == Some(profile_id) && a.id != id) {
            if !other.rules.contains(rule) {
                other.rules.push(rule.clone());
                self.broker.set_rules(RuleScope::Agent(other.id), other.rules.clone());
                if let Some(store) = &self.store {
                    store.save_agent(other);
                }
            }
        }
    }
}
