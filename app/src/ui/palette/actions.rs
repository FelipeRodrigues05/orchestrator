//! What Enter does: free text submitted in a wizard step, or the picked item.

use orch_core::orchestrator::Command;
use uuid::Uuid;

use super::{Mode, Outcome, Palette, Pick, Step, Wizard};

impl Palette {
    pub(super) fn submit_text(&mut self, text: String) -> Outcome {
        match &mut self.mode {
            Mode::NewAgent(wizard) => match wizard.step {
                Step::GroupGoal => {
                    let name = wizard.new_group.take().unwrap_or_default();
                    wizard.group = Some(name.clone());
                    wizard.step = Step::Name;
                    self.query.clear();
                    Outcome::Send(Command::CreateGroup { name, goal: text })
                }
                Step::Name => {
                    wizard.name = Some(text);
                    wizard.step = Step::Provider;
                    self.query.clear();
                    self.cursor = 0;
                    Outcome::Keep
                }
                Step::Task => {
                    wizard.task = Some(text);
                    wizard.step = Step::Persona;
                    self.query.clear();
                    Outcome::Keep
                }
                _ => Outcome::Keep,
            },
            Mode::NewGroup { name } => match name.take() {
                None => {
                    *name = Some(text);
                    self.query.clear();
                    Outcome::Keep
                }
                Some(name) => Outcome::Done(Command::CreateGroup { name, goal: text }),
            },
            _ => Outcome::Keep,
        }
    }

    pub(super) fn pick(&mut self, pick: Pick, selected: Option<Uuid>) -> Outcome {
        match pick {
            Pick::NewAgent => {
                self.to(Mode::NewAgent(Wizard::default()));
                Outcome::Keep
            }
            Pick::NewGroup => {
                self.to(Mode::NewGroup { name: None });
                Outcome::Keep
            }
            Pick::OpenProject => {
                self.to(Mode::OpenProject);
                Outcome::Keep
            }
            Pick::ApiKey => {
                self.to(Mode::ApiKey);
                Outcome::Keep
            }
            Pick::OpenIn => {
                self.to(Mode::OpenIn);
                Outcome::Keep
            }
            Pick::OpenProjectIn => Outcome::Done(Command::OpenIn {
                agent: None,
                editor: None,
            }),
            Pick::Toggle(id, true) => Outcome::Done(Command::StopAgent(id)),
            Pick::Toggle(id, false) => Outcome::Done(Command::StartAgent(id)),
            Pick::Message => Outcome::Message,
            Pick::Remove(id) => Outcome::Done(Command::RemoveAgent(id)),
            Pick::View(view) => Outcome::View(view),
            Pick::GoTo(id) => Outcome::Select(id),
            Pick::FocusProject(id) => Outcome::Done(Command::FocusProject(id)),
            Pick::CloseProject(id) => Outcome::Done(Command::CloseProject(id)),
            Pick::OpenPath(root) => Outcome::Done(Command::OpenProject { root }),
            Pick::Browse => Outcome::Browse,
            Pick::Insert(text) => Outcome::Insert(text),
            Pick::ChooseModel(agent) => {
                self.to(Mode::Model(agent));
                // The list fills in as the provider answers.
                Outcome::Send(Command::ProbeModels(agent))
            }
            Pick::Waiting => Outcome::Keep,
            Pick::ChooseTheme => {
                self.to(Mode::Theme);
                Outcome::Keep
            }
            Pick::Theme(name) => Outcome::Done(Command::SetTheme(name.to_owned())),
            Pick::DeleteKeys => {
                self.to(Mode::DeleteKey);
                Outcome::Keep
            }
            Pick::DeleteKey(service) => Outcome::Done(Command::DeleteApiKey(service)),
            Pick::ChooseColor(agent) => {
                self.to(Mode::Color(agent));
                Outcome::Keep
            }
            Pick::Color(color) => match self.mode {
                Mode::Color(agent) => Outcome::Done(Command::SetColor { agent, color }),
                _ => Outcome::Keep,
            },
            Pick::Model(model) => match self.mode {
                Mode::Model(agent) => Outcome::Done(Command::SetModel { agent, model }),
                _ => Outcome::Keep,
            },
            Pick::PersonaText(persona) => match &self.mode {
                Mode::NewAgent(wizard) => Outcome::Done(Command::CreateAgent {
                    group: wizard.group.clone().unwrap_or_default(),
                    name: wizard.name.clone().unwrap_or_default(),
                    provider: wizard.provider.clone(),
                    task: wizard.task.clone().unwrap_or_default(),
                    persona,
                }),
                Mode::Persona(agent) => Outcome::Done(Command::SetPersona {
                    agent: *agent,
                    persona,
                }),
                _ => Outcome::Keep,
            },
            Pick::Persona(agent) => {
                self.to(Mode::Persona(agent));
                Outcome::Keep
            }
            Pick::Help => Outcome::Help,
            Pick::Group(name) => {
                if let Mode::NewAgent(wizard) = &mut self.mode {
                    wizard.group = Some(name);
                    wizard.step = Step::Name;
                }
                self.query.clear();
                self.cursor = 0;
                Outcome::Keep
            }
            Pick::CreateGroup(name) => {
                if let Mode::NewAgent(wizard) = &mut self.mode {
                    wizard.new_group = Some(name);
                    wizard.step = Step::GroupGoal;
                }
                self.query.clear();
                Outcome::Keep
            }
            Pick::Provider(label) => {
                if let Mode::SwitchProvider(agent) = self.mode {
                    return Outcome::Done(Command::SetProvider {
                        agent,
                        provider: label,
                    });
                }
                if self.mode == Mode::PetModel {
                    return Outcome::Done(Command::SetHostProvider(label));
                }
                if let Mode::NewAgent(wizard) = &mut self.mode {
                    wizard.provider = Some(label);
                    wizard.step = Step::Task;
                }
                self.query.clear();
                self.cursor = 0;
                Outcome::Keep
            }
            Pick::Service(service) => Outcome::Secret(service),
            Pick::Editor(name) => Outcome::Done(Command::OpenIn {
                agent: selected,
                editor: Some(name),
            }),
            Pick::Run(text) => Outcome::Run(text),
            Pick::SwitchProvider(agent) => {
                self.to(Mode::SwitchProvider(agent));
                Outcome::Keep
            }
            Pick::UpdateClis => Outcome::Done(Command::UpdateClis),
            Pick::InstallVoice => Outcome::Done(Command::InstallVoice),
            Pick::Merge(agent) => Outcome::Done(Command::Merge(agent)),
            Pick::Language(code) => Outcome::Done(Command::SetLanguage(code)),
            Pick::ChooseVoice => {
                self.to(Mode::Voice);
                Outcome::Keep
            }
            Pick::Voice(id) => Outcome::Done(Command::SetVoice(id)),
            Pick::OpenEmail => Outcome::Done(Command::OpenEmail),
            Pick::OpenTerminal => Outcome::Done(Command::OpenTerminal(selected)),
            Pick::Lead(group) => Outcome::Done(Command::Lead { group, agents: 3 }),
            Pick::PetModel => {
                self.to(Mode::PetModel);
                Outcome::Keep
            }
        }
    }
}
