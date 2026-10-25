//! What the account's plan affords: config [models], set once at startup. Summon's choices and defaults, adopt and
//! the dispatcher's compaction point follow it; without the section, a Max plan's (opus, up to 1m of context).

use std::sync::OnceLock;

use crate::workers::{CONTEXTS, MODELS};

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// For real work, and summon's model when none is given.
    pub work: String,
    /// For routine jobs.
    pub routine: String,
    /// For lookups, and every shikigami's own subagents.
    pub quick: String,
    /// The largest context a shikigami gets, in tokens.
    pub max_context: f64,
}

impl Default for Plan {
    fn default() -> Plan {
        Plan { work: "opus".into(), routine: "sonnet".into(), quick: "haiku".into(), max_context: 1_000_000.0 }
    }
}

static PLAN: OnceLock<Plan> = OnceLock::new();

/// The plan in config [models]; what is missing or not understood keeps the default, with a line in the log.
pub fn from_config(models: Option<&toml::Table>) -> Plan {
    let mut p = Plan::default();
    let Some(t) = models else { return p };
    for (key, slot) in [("work", &mut p.work), ("routine", &mut p.routine), ("quick", &mut p.quick)] {
        match t.get(key).and_then(|v| v.as_str()) {
            Some(m) if MODELS.contains(&m) => *slot = m.into(),
            Some(m) => eprintln!("seimei: [models] {key} = {m:?} is not one of {}; using {slot}", MODELS.join(", ")),
            None => {}
        }
    }
    if let Some(c) = t.get("context").and_then(|v| v.as_str()) {
        match CONTEXTS.iter().find(|x| x.0 == c) {
            Some(x) => p.max_context = x.1,
            None => eprintln!("seimei: [models] context = {c:?} is not one of {}", CONTEXTS.map(|x| x.0).join(", ")),
        }
    }
    p
}

/// Sets the plan for the life of the process (the first call wins).
pub fn set(p: Plan) {
    let _ = PLAN.set(p);
}

pub fn get() -> &'static Plan {
    PLAN.get_or_init(Plan::default)
}

impl Plan {
    /// The contexts a shikigami may get, smallest first.
    pub fn contexts(&self) -> Vec<&'static str> {
        CONTEXTS.iter().filter(|c| c.1 <= self.max_context).map(|c| c.0).collect()
    }

    /// How summon's model choice reads.
    pub fn model_guide(&self) -> String {
        if self.work == self.routine {
            format!("{} for real work and routine jobs, {} for quick lookups", self.work, self.quick)
        } else {
            format!("{} for real work, {} for routine jobs, {} for quick lookups", self.work, self.routine, self.quick)
        }
    }

    /// How summon's context choice reads.
    pub fn context_guide(&self) -> String {
        match self.contexts()[..] {
            [only] => format!("where it compacts: {only}, the most the plan's models hold"),
            [a, b] => format!("where it compacts: {a} for most tasks; {b} only when the task really needs it"),
            [ref most @ .., last] => {
                let most = most.iter().skip(most.len().saturating_sub(2)).copied().collect::<Vec<_>>().join(" or ");
                format!("where it compacts: {most} for most tasks; {last} only when the task really needs it")
            }
            [] => String::new(),
        }
    }

    /// The smallest context with room above what a conversation holds already, else the largest.
    pub fn context_for(&self, tokens: i64) -> &'static str {
        let mut fits = CONTEXTS.iter().filter(|c| c.1 <= self.max_context);
        let room = fits.clone().find(|c| (tokens as f64) < c.1 * 0.7);
        room.or(fits.next_back()).map_or("200k", |c| c.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pro_plan_offers_sonnet_and_200k() {
        let max = Plan::default();
        assert_eq!(max.contexts(), ["200k", "256k", "400k", "1m"]);
        assert_eq!(
            max.context_guide(),
            "where it compacts: 256k or 400k for most tasks; 1m only when the task really needs it"
        );
        assert_eq!(
            (max.context_for(150_000), max.context_for(250_000), max.context_for(600_000)),
            ("256k", "400k", "1m")
        );
        let t: toml::Table = "work = \"sonnet\"\ncontext = \"200k\"\nquick = \"gpt\"".parse().unwrap();
        let pro = from_config(Some(&t));
        assert_eq!((pro.work.as_str(), pro.quick.as_str(), pro.contexts()), ("sonnet", "haiku", vec!["200k"]));
        assert_eq!(pro.model_guide(), "sonnet for real work and routine jobs, haiku for quick lookups");
        assert_eq!(pro.context_for(500_000), "200k");
    }
}
