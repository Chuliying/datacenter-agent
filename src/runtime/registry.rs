//! Builtin runtime registry.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::audit::{AuditSink, TracingAuditSink};
use super::config::RuntimeConfig;
use super::error::{RuntimeError, RuntimeResult};
use super::eval::evaluator::{Evaluator, NoopEvaluator};
use super::guardrails::answer_policy::{AnswerPolicy, RuleAnswerPolicy};
use super::llm_normalizer::{DisabledLlmNormalizer, LlmInputNormalizer};
use super::memory::store::{InMemorySessionStore, SessionMemoryStore};

/// Registry of builtin module ids.
#[derive(Debug, Clone)]
pub struct BuiltinRegistry {
    input_stages: BTreeSet<&'static str>,
    answer_policies: BTreeSet<&'static str>,
    llm_normalizers: BTreeSet<&'static str>,
    memory_backends: BTreeSet<&'static str>,
    audit_sinks: BTreeSet<&'static str>,
    guardrails: BTreeSet<&'static str>,
    slot_extractors: BTreeSet<&'static str>,
    evaluators: BTreeSet<&'static str>,
}

impl Default for BuiltinRegistry {
    fn default() -> Self {
        Self {
            input_stages: BTreeSet::from([
                "normalize",
                "input_guard",
                "injection",
                "intent",
                "slots",
            ]),
            answer_policies: BTreeSet::from(["rule"]),
            llm_normalizers: BTreeSet::from(["disabled"]),
            memory_backends: BTreeSet::from(["in-memory"]),
            audit_sinks: BTreeSet::from(["tracing"]),
            guardrails: BTreeSet::from(["injection", "input_guard", "answer_policy"]),
            slot_extractors: BTreeSet::from(["time_range", "metric", "asset", "rank_limit"]),
            evaluators: BTreeSet::from([
                "pipeline-deterministic",
                "response-baseline",
                "llm-judge",
            ]),
        }
    }
}

impl BuiltinRegistry {
    /// Return whether an input stage id is known.
    pub fn has_input_stage(&self, id: &str) -> bool {
        self.input_stages.contains(id)
    }

    /// Validate a stage id.
    pub fn require_input_stage(&self, id: &str, section: &str) -> RuntimeResult<()> {
        require_known(&self.input_stages, id, section)
    }

    /// Validate an answer policy backend id.
    pub fn require_answer_policy(&self, id: &str, section: &str) -> RuntimeResult<()> {
        require_known(&self.answer_policies, id, section)
    }

    /// Validate an LLM normalizer backend id.
    pub fn require_llm_normalizer(&self, id: &str, section: &str) -> RuntimeResult<()> {
        require_known(&self.llm_normalizers, id, section)
    }

    /// Validate a memory backend id.
    pub fn require_memory_backend(&self, id: &str, section: &str) -> RuntimeResult<()> {
        require_known(&self.memory_backends, id, section)
    }

    /// Validate an audit sink id.
    pub fn require_audit_sink(&self, id: &str, section: &str) -> RuntimeResult<()> {
        require_known(&self.audit_sinks, id, section)
    }

    /// Validate a guardrail id.
    pub fn require_guardrail(&self, id: &str, section: &str) -> RuntimeResult<()> {
        require_known(&self.guardrails, id, section)
    }

    /// Validate a slot extractor id.
    pub fn require_slot_extractor(&self, id: &str, section: &str) -> RuntimeResult<()> {
        require_known(&self.slot_extractors, id, section)
    }

    /// Validate an evaluator id.
    pub fn require_evaluator(&self, id: &str, section: &str) -> RuntimeResult<()> {
        require_known(&self.evaluators, id, section)
    }

    /// Build configured input stage ids in order.
    pub fn build_input_pipeline(&self, cfg: &RuntimeConfig) -> RuntimeResult<Vec<String>> {
        for stage in &cfg.assembly.input_stages {
            self.require_input_stage(stage, "runtime.input")?;
        }
        Ok(cfg.assembly.input_stages.clone())
    }

    /// Build the configured answer policy.
    pub fn build_answer_policy(&self, cfg: &RuntimeConfig) -> RuntimeResult<Arc<dyn AnswerPolicy>> {
        self.require_answer_policy(&cfg.assembly.answer_policy_backend, "runtime.answer_policy")?;
        Ok(Arc::new(RuleAnswerPolicy::new(&cfg.thresholds.confidence)))
    }

    /// Build the optional LLM normalizer.
    pub fn build_llm_normalizer(
        &self,
        cfg: &RuntimeConfig,
    ) -> RuntimeResult<Option<Arc<dyn LlmInputNormalizer>>> {
        self.require_llm_normalizer(
            &cfg.assembly.llm_normalizer_backend,
            "runtime.llm_normalizer",
        )?;
        if cfg.assembly.llm_normalizer_enabled {
            Ok(Some(Arc::new(DisabledLlmNormalizer)))
        } else {
            Ok(None)
        }
    }

    /// Build the optional memory store.
    pub fn build_memory(
        &self,
        cfg: &RuntimeConfig,
    ) -> RuntimeResult<Option<Arc<dyn SessionMemoryStore>>> {
        self.require_memory_backend(&cfg.assembly.memory_backend, "runtime.memory")?;
        if cfg.assembly.memory_enabled {
            Ok(Some(Arc::new(InMemorySessionStore::new(
                cfg.thresholds.memory.max_turns,
            ))))
        } else {
            Ok(None)
        }
    }

    /// Build the configured audit sink.
    pub fn build_audit(&self, cfg: &RuntimeConfig) -> RuntimeResult<Arc<dyn AuditSink>> {
        self.require_audit_sink(&cfg.assembly.audit_sink, "runtime.audit")?;
        Ok(Arc::new(TracingAuditSink))
    }

    /// Build configured evaluator ids.
    pub fn build_evaluators(&self, cfg: &RuntimeConfig) -> RuntimeResult<Vec<Arc<dyn Evaluator>>> {
        let mut evaluators: Vec<Arc<dyn Evaluator>> = Vec::new();
        for id in cfg
            .assembly
            .pipeline_evaluators
            .iter()
            .chain(cfg.assembly.response_evaluators.iter())
        {
            self.require_evaluator(id, "runtime.eval")?;
            evaluators.push(Arc::new(NoopEvaluator::new(id.clone())));
        }
        Ok(evaluators)
    }
}

fn require_known(known: &BTreeSet<&'static str>, id: &str, section: &str) -> RuntimeResult<()> {
    if known.contains(id) {
        Ok(())
    } else {
        Err(RuntimeError::UnknownModule {
            id: id.to_string(),
            section: section.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AppConfig;
    use crate::runtime::config::RuntimeConfig;

    use super::*;

    fn default_runtime_config() -> RuntimeConfig {
        let refs = AppConfig::load("config/config.toml")
            .expect("app config should load")
            .runtime
            .expect("runtime refs should exist");
        RuntimeConfig::load(&refs, &BuiltinRegistry::default()).expect("runtime config should load")
    }

    #[test]
    fn builds_builtin_runtime_components() {
        let registry = BuiltinRegistry::default();
        let cfg = default_runtime_config();

        assert_eq!(
            registry
                .build_input_pipeline(&cfg)
                .expect("pipeline should build"),
            ["normalize", "input_guard", "injection", "intent", "slots"]
        );
        assert!(registry.build_answer_policy(&cfg).is_ok());
        assert!(registry
            .build_llm_normalizer(&cfg)
            .expect("normalizer should build")
            .is_none());
        assert!(registry
            .build_memory(&cfg)
            .expect("memory should build")
            .is_some());
        assert!(registry.build_audit(&cfg).is_ok());
        assert_eq!(
            registry
                .build_evaluators(&cfg)
                .expect("evaluators should build")
                .len(),
            3
        );
    }
}
