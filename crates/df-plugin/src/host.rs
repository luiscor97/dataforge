use std::collections::{BTreeMap, BTreeSet};
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::Duration;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{Component, Linker, Type};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use crate::contract::{
    output_schema, AnalysisRequest, Capability, PluginInput, PluginOutput, INPUT_SCHEMA_VERSION,
};
use crate::error::{LimitKind, PluginError, PluginResult};
use crate::registry::{PluginKey, PluginRegistry, RegisteredPluginMetadata, SignedPluginPackage};

const HARD_MAX_COMPONENT_BYTES: u64 = 32 * 1024 * 1024;
const HARD_MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const HARD_MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;
const HARD_MAX_FUEL: u64 = 100_000_000;
const HARD_MAX_TIMEOUT_MS: u64 = 10_000;
const HARD_MAX_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
const HARD_MAX_TABLE_ELEMENTS: u32 = 100_000;
const HARD_MAX_INSTANCES: u32 = 100;
const HARD_MAX_TABLES: u32 = 100;
const HARD_MAX_MEMORIES: u32 = 100;
const HARD_MAX_WASM_STACK_BYTES: u64 = 4 * 1024 * 1024;

/// Hard-ceiling-bounded execution policy. Callers may lower these values but
/// cannot disable the resource boundary with `0` or an effectively unbounded
/// integer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostLimits {
    pub max_component_bytes: u64,
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    pub fuel: u64,
    pub epoch_timeout_ms: u64,
    pub max_memory_bytes: u64,
    pub max_table_elements: u32,
    pub max_instances: u32,
    pub max_tables: u32,
    pub max_memories: u32,
    pub max_wasm_stack_bytes: u64,
}

impl Default for HostLimits {
    fn default() -> Self {
        Self {
            max_component_bytes: 8 * 1024 * 1024,
            max_input_bytes: 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            fuel: 10_000_000,
            epoch_timeout_ms: 500,
            max_memory_bytes: 32 * 1024 * 1024,
            max_table_elements: 10_000,
            max_instances: 16,
            max_tables: 16,
            max_memories: 16,
            max_wasm_stack_bytes: 512 * 1024,
        }
    }
}

/// Capabilities an operator explicitly grants to this host instance.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostPolicy {
    pub granted_capabilities: BTreeSet<Capability>,
}

struct StoreState {
    limits: StoreLimits,
}

/// Empty-linker Component Model host with signed registration and bounded,
/// serialized execution. Serialization prevents a global epoch tick for one
/// invocation from interrupting another invocation early.
pub struct PluginHost {
    engine: Engine,
    limits: HostLimits,
    policy: HostPolicy,
    registry: PluginRegistry,
    compiled: BTreeMap<PluginKey, Component>,
    execution_lock: Mutex<()>,
}

impl PluginHost {
    pub fn new(limits: HostLimits, policy: HostPolicy) -> PluginResult<Self> {
        validate_limits(&limits)?;
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(usize_limit(limits.max_wasm_stack_bytes, "Wasm stack")?);
        let engine = Engine::new(&config)
            .map_err(|error| PluginError::InvalidHostConfiguration(error.to_string()))?;
        Ok(Self {
            engine,
            limits,
            policy,
            registry: PluginRegistry::new(),
            compiled: BTreeMap::new(),
            execution_lock: Mutex::new(()),
        })
    }

    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    pub fn registered_plugins(&self) -> Vec<RegisteredPluginMetadata> {
        self.registry.metadata()
    }

    /// Cryptographically verify, compile and type-check before atomically
    /// appending the immutable entry to the registry.
    pub fn register(&mut self, package: SignedPluginPackage) -> PluginResult<PluginKey> {
        let mut candidate = self.registry.clone();
        let max_component_bytes =
            usize_limit(self.limits.max_component_bytes, "maximum component bytes")?;
        let key = candidate.register(package, max_component_bytes)?;
        let verified = candidate
            .get(&key)
            .expect("a successful candidate registration returns its inserted key");
        let component = Component::new(&self.engine, verified.component_bytes())
            .map_err(|error| PluginError::InvalidComponent(error.to_string()))?;
        validate_component_contract(&self.engine, &component)?;

        self.registry = candidate;
        self.compiled.insert(key.clone(), component);
        Ok(key)
    }

    /// Return the component's informational description under the same fuel,
    /// epoch, memory and output-size boundaries as analysis.
    pub fn describe(&self, key: &PluginKey) -> PluginResult<String> {
        let _guard = self
            .execution_lock
            .lock()
            .map_err(|_| PluginError::RuntimeTrap)?;
        self.ensure_grants(key)?;
        let component = self.component(key)?;
        let (mut store, instance) = self.instantiate(component)?;
        let function = instance
            .get_typed_func::<(), (String,)>(&mut store, "describe")
            .map_err(|error| PluginError::InvalidComponent(error.to_string()))?;
        let (result,) = self.call_with_epoch(|| function.call(&mut store, ()))?;
        function
            .post_return(&mut store)
            .map_err(classify_runtime_error)?;
        self.check_output_size(result.as_bytes())?;
        Ok(result)
    }

    /// Invoke `analyze`. Only explicitly requested-and-granted data is copied
    /// into the plugin input; there are no ambient host imports.
    pub fn analyze(
        &self,
        key: &PluginKey,
        request: &AnalysisRequest,
    ) -> PluginResult<PluginOutput> {
        let _guard = self
            .execution_lock
            .lock()
            .map_err(|_| PluginError::RuntimeTrap)?;
        let input = self.filtered_input(key, request)?;
        let input_json = serde_json::to_string(&input)
            .map_err(|error| PluginError::InvalidComponent(error.to_string()))?;
        if u64::try_from(input_json.len()).unwrap_or(u64::MAX) > self.limits.max_input_bytes {
            return Err(PluginError::LimitExceeded {
                kind: LimitKind::InputBytes,
            });
        }

        let component = self.component(key)?;
        let (mut store, instance) = self.instantiate(component)?;
        let function = instance
            .get_typed_func::<(String,), (String,)>(&mut store, "analyze")
            .map_err(|error| PluginError::InvalidComponent(error.to_string()))?;
        let (raw_output,) = self.call_with_epoch(|| function.call(&mut store, (input_json,)))?;
        function
            .post_return(&mut store)
            .map_err(classify_runtime_error)?;
        self.check_output_size(raw_output.as_bytes())?;
        validate_output(&raw_output)
    }

    /// Build the exact immutable snapshot a plugin will receive. Exposing this
    /// pure view makes capability filtering auditable without executing guest
    /// code.
    pub fn filtered_input(
        &self,
        key: &PluginKey,
        request: &AnalysisRequest,
    ) -> PluginResult<PluginInput> {
        let capabilities = self.ensure_grants(key)?;
        Ok(PluginInput {
            schema_version: INPUT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id.clone(),
            subject: request.subject.clone(),
            metadata: capabilities
                .contains(&Capability::SubjectMetadata)
                .then(|| request.metadata.clone()),
            normalized_text: capabilities
                .contains(&Capability::SubjectText)
                .then(|| request.normalized_text.clone())
                .flatten(),
        })
    }

    fn component(&self, key: &PluginKey) -> PluginResult<&Component> {
        self.compiled
            .get(key)
            .ok_or_else(|| PluginError::NotRegistered(key.to_string()))
    }

    fn ensure_grants(&self, key: &PluginKey) -> PluginResult<&BTreeSet<Capability>> {
        let plugin = self
            .registry
            .get(key)
            .ok_or_else(|| PluginError::NotRegistered(key.to_string()))?;
        let requested = &plugin.metadata().manifest.capabilities;
        if !requested.is_subset(&self.policy.granted_capabilities) {
            let denied = requested
                .difference(&self.policy.granted_capabilities)
                .map(|capability| format!("{capability:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(PluginError::CapabilityDenied(format!(
                "signed manifest requests ungranted capabilities: {denied}"
            )));
        }
        Ok(requested)
    }

    fn instantiate(
        &self,
        component: &Component,
    ) -> PluginResult<(Store<StoreState>, wasmtime::component::Instance)> {
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(usize_limit(self.limits.max_memory_bytes, "memory bytes")?)
            .table_elements(self.limits.max_table_elements as usize)
            .instances(self.limits.max_instances as usize)
            .tables(self.limits.max_tables as usize)
            .memories(self.limits.max_memories as usize)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(
            &self.engine,
            StoreState {
                limits: store_limits,
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(self.limits.fuel)
            .map_err(classify_runtime_error)?;
        store.set_epoch_deadline(1);

        // No host functions and no wasmtime-wasi dependency: the empty linker
        // is the ambient-authority boundary for ABI 0.1.
        let linker = Linker::<StoreState>::new(&self.engine);
        let instance = linker
            .instantiate(&mut store, component)
            .map_err(classify_runtime_error)?;
        Ok((store, instance))
    }

    fn call_with_epoch<T>(&self, call: impl FnOnce() -> wasmtime::Result<T>) -> PluginResult<T> {
        let (cancel_tx, cancel_rx) = mpsc::sync_channel::<()>(1);
        let engine = self.engine.clone();
        let timeout = Duration::from_millis(self.limits.epoch_timeout_ms);
        let timer = thread::Builder::new()
            .name("df-plugin-epoch-guard".to_string())
            .spawn(move || {
                if cancel_rx.recv_timeout(timeout).is_err() {
                    engine.increment_epoch();
                }
            })
            .map_err(|error| PluginError::InvalidHostConfiguration(error.to_string()))?;
        let result = call().map_err(classify_runtime_error);
        let _ = cancel_tx.send(());
        let _ = timer.join();
        result
    }

    fn check_output_size(&self, bytes: &[u8]) -> PluginResult<()> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > self.limits.max_output_bytes {
            return Err(PluginError::LimitExceeded {
                kind: LimitKind::OutputBytes,
            });
        }
        Ok(())
    }
}

fn validate_component_contract(engine: &Engine, component: &Component) -> PluginResult<()> {
    let component_type = component.component_type();
    if let Some((name, _)) = component_type.imports(engine).next() {
        return Err(PluginError::CapabilityDenied(format!(
            "ABI 0.1 exposes no host imports; component imports `{name}`"
        )));
    }
    validate_export(&component_type, engine, "describe", &[], &[Type::String])?;
    validate_export(
        &component_type,
        engine,
        "analyze",
        &[Type::String],
        &[Type::String],
    )?;
    Ok(())
}

fn validate_export(
    component_type: &wasmtime::component::types::Component,
    engine: &Engine,
    name: &str,
    expected_params: &[Type],
    expected_results: &[Type],
) -> PluginResult<()> {
    let function = match component_type.get_export(engine, name) {
        Some(ComponentItem::ComponentFunc(function)) => function,
        _ => {
            return Err(PluginError::InvalidComponent(format!(
                "required component function export `{name}` is absent"
            )))
        }
    };
    let params = function.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    let results = function.results().collect::<Vec<_>>();
    if params != expected_params || results != expected_results {
        return Err(PluginError::InvalidComponent(format!(
            "component function `{name}` does not match the WIT ABI"
        )));
    }
    Ok(())
}

fn validate_output(raw: &str) -> PluginResult<PluginOutput> {
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let StrictJson(value) = StrictJson::deserialize(&mut deserializer)
        .map_err(|error| PluginError::MalformedOutput(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| PluginError::MalformedOutput(error.to_string()))?;
    if value
        .get("automatic_action")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Err(PluginError::AutomaticActionForbidden);
    }
    let schema = output_schema();
    let validator = jsonschema::draft202012::new(&schema)
        .map_err(|error| PluginError::OutputSchema(error.to_string()))?;
    if let Err(error) = validator.validate(&value) {
        return Err(PluginError::OutputSchema(error.to_string()));
    }
    let output: PluginOutput = serde_json::from_value(value)
        .map_err(|error| PluginError::MalformedOutput(error.to_string()))?;
    if output.automatic_action {
        return Err(PluginError::AutomaticActionForbidden);
    }
    Ok(output)
}

fn classify_runtime_error(error: wasmtime::Error) -> PluginError {
    if matches!(
        error.downcast_ref::<wasmtime::Trap>(),
        Some(wasmtime::Trap::OutOfFuel | wasmtime::Trap::Interrupt)
    ) {
        return PluginError::LimitExceeded {
            kind: LimitKind::FuelOrEpoch,
        };
    }
    let message = error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
        .to_ascii_lowercase();
    if message.contains("fuel")
        || message.contains("epoch")
        || message.contains("interrupt")
        || message.contains("deadline")
    {
        PluginError::LimitExceeded {
            kind: LimitKind::FuelOrEpoch,
        }
    } else if message.contains("memory") || message.contains("grow") {
        PluginError::LimitExceeded {
            kind: LimitKind::Memory,
        }
    } else if message.contains("resource limit")
        || message.contains("instance")
        || message.contains("table")
    {
        PluginError::LimitExceeded {
            kind: LimitKind::RuntimeResources,
        }
    } else if message.contains("import") || message.contains("linker") {
        PluginError::CapabilityDenied(
            "the component requested a host import that ABI 0.1 does not expose".to_string(),
        )
    } else {
        PluginError::RuntimeTrap
    }
}

/// `serde_json::Value` accepts duplicate object names with last-value-wins
/// semantics. That would let an auditor see a different `automatic_action` or
/// `findings` member than the host. This recursive visitor rejects duplicates
/// at every object depth before JSON Schema validation.
struct StrictJson(serde_json::Value);

impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJson;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object member names")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Null))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(|number| StrictJson(serde_json::Value::Number(number)))
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictJson(serde_json::Value::String(value)))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(256));
        while let Some(StrictJson(value)) = sequence.next_element::<StrictJson>()? {
            values.push(value);
        }
        Ok(StrictJson(serde_json::Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object member `{key}`"
                )));
            }
            let StrictJson(value) = object.next_value::<StrictJson>()?;
            values.insert(key, value);
        }
        Ok(StrictJson(serde_json::Value::Object(values)))
    }
}

fn validate_limits(limits: &HostLimits) -> PluginResult<()> {
    validate_limit(
        "max_component_bytes",
        limits.max_component_bytes,
        HARD_MAX_COMPONENT_BYTES,
    )?;
    validate_limit(
        "max_input_bytes",
        limits.max_input_bytes,
        HARD_MAX_INPUT_BYTES,
    )?;
    validate_limit(
        "max_output_bytes",
        limits.max_output_bytes,
        HARD_MAX_OUTPUT_BYTES,
    )?;
    validate_limit("fuel", limits.fuel, HARD_MAX_FUEL)?;
    validate_limit(
        "epoch_timeout_ms",
        limits.epoch_timeout_ms,
        HARD_MAX_TIMEOUT_MS,
    )?;
    validate_limit(
        "max_memory_bytes",
        limits.max_memory_bytes,
        HARD_MAX_MEMORY_BYTES,
    )?;
    validate_limit(
        "max_table_elements",
        u64::from(limits.max_table_elements),
        u64::from(HARD_MAX_TABLE_ELEMENTS),
    )?;
    validate_limit(
        "max_instances",
        u64::from(limits.max_instances),
        u64::from(HARD_MAX_INSTANCES),
    )?;
    validate_limit(
        "max_tables",
        u64::from(limits.max_tables),
        u64::from(HARD_MAX_TABLES),
    )?;
    validate_limit(
        "max_memories",
        u64::from(limits.max_memories),
        u64::from(HARD_MAX_MEMORIES),
    )?;
    validate_limit(
        "max_wasm_stack_bytes",
        limits.max_wasm_stack_bytes,
        HARD_MAX_WASM_STACK_BYTES,
    )?;
    Ok(())
}

fn validate_limit(name: &str, value: u64, hard_max: u64) -> PluginResult<()> {
    if value == 0 || value > hard_max {
        return Err(PluginError::InvalidHostConfiguration(format!(
            "{name} must be between 1 and {hard_max}"
        )));
    }
    Ok(())
}

fn usize_limit(value: u64, name: &str) -> PluginResult<usize> {
    usize::try_from(value).map_err(|_| {
        PluginError::InvalidHostConfiguration(format!(
            "{name} is not representable on this platform"
        ))
    })
}
