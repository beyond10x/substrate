//! The closed resolved plan shared verbatim with generated runtimes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A checked JSON value shape in the supported projection subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    /// UTF-8 text.
    String,
    /// A boolean.
    Boolean,
    /// An exact JSON integer token in the model's signed 64-bit range.
    Integer,
    /// Any JSON value, written as JSON. Only a result carries it.
    Json,
    /// One declared string variant.
    Enum {
        /// Permitted values.
        variants: Vec<String>,
    },
    /// A nullable or absent value.
    Optional {
        /// The present value.
        of: Box<Shape>,
    },
    /// An ordered sequence.
    List {
        /// Item shape.
        of: Box<Shape>,
    },
    /// String-keyed values.
    Map {
        /// Value shape.
        value: Box<Shape>,
    },
    /// An object with closed fields.
    Struct {
        /// Wire fields.
        fields: BTreeMap<String, Shape>,
    },
}

impl Shape {
    /// Whether omission is allowed.
    pub fn optional(&self) -> bool {
        matches!(self, Self::Optional { .. })
    }
    /// The underlying non-optional shape.
    pub fn required(&self) -> &Self {
        if let Self::Optional { of } = self {
            of.required()
        } else {
            self
        }
    }
    /// Check an instance without exposing values in diagnostics.
    pub fn accepts(&self, value: &serde_json::Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Boolean => value.is_boolean(),
            Self::Integer => value.is_i64(),
            // A parsed value is well-formed JSON; what surrounds it is checked by its container.
            Self::Json => true,
            Self::Enum { variants } => value
                .as_str()
                .is_some_and(|s| variants.iter().any(|v| v == s)),
            Self::Optional { of } => value.is_null() || of.accepts(value),
            Self::List { of } => value
                .as_array()
                .is_some_and(|a| a.iter().all(|v| of.accepts(v))),
            Self::Map { value: shape } => value
                .as_object()
                .is_some_and(|o| o.values().all(|v| shape.accepts(v))),
            Self::Struct { fields } => value.as_object().is_some_and(|object| {
                object.keys().all(|key| fields.contains_key(key))
                    && fields.iter().all(|(name, shape)| {
                        object
                            .get(name)
                            .map_or_else(|| shape.optional(), |v| shape.accepts(v))
                    })
            }),
        }
    }
}

/// Process selectors, separate from every callable payload.
///
/// `ess-cli-plan/1` carries all three. `ess-cli-plan/2` omits an undeclared `config` or `output`
/// key and never writes `null` for one, so an `ess-cli-plan/1` plan keeps its bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Globals {
    /// Configuration file selector; absent, the CLI has no configuration flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// State-directory selector.
    pub state: String,
    /// Human/JSON output selector; absent, the CLI writes JSON only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// Owner-qualified callable dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// Application-local interface metadata.
    Local {
        /// Qualified application owner metadata.
        owner: String,
        /// Local action identity.
        action: String,
    },
    /// Forward to an operation whose model owner is unchanged.
    ServiceForward {
        /// Existing component identity.
        owner: String,
        /// Existing command or view identity.
        operation: String,
    },
    /// Schema-selected dynamic operation with a typed outer declaration.
    Dynamic {
        /// Qualified application owner metadata.
        owner: String,
        /// String field containing the operation identity.
        operation_field: String,
        /// String field containing the native schema identity.
        schema_field: String,
        /// String field containing JSON payload text.
        payload_field: String,
    },
}

/// An argv mapping or protected acquisition channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArgumentSource {
    /// Ordinary flag taking one value.
    Option {
        /// Long flag name, without dashes.
        long: String,
    },
    /// Ordinary positional value.
    Positional {
        /// Consecutive one-based index.
        index: usize,
    },
    /// A bounded nonsecret UTF-8 document from one explicit source.
    Document {
        /// Flag taking inline document text.
        inline: String,
        /// Flag taking a document file path.
        file: String,
        /// Boolean flag selecting stdin.
        stdin: String,
    },
    /// A protected String with mutually exclusive acquisition sources.
    Protected {
        /// Flag taking a file path.
        file: String,
        /// Boolean flag selecting stdin.
        stdin: String,
        /// Boolean flag selecting a hidden TTY prompt.
        hidden_tty: String,
    },
    /// Every argv word after the first `--`, verbatim, into one required `List<String>`.
    Trailing {},
}

/// One input field's source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Argument {
    /// ESS field name (normalized to its wire name after resolution).
    pub field: String,
    /// The source selected for this field.
    pub source: ArgumentSource,
}

/// One root or one-group command and finite path aliases.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// Primary command path.
    pub path: Vec<String>,
    /// Explicit alternative paths.
    #[serde(default)]
    pub aliases: Vec<Vec<String>>,
    /// Declared callable key.
    pub callable: String,
    /// Human help summary.
    pub about: String,
    /// Complete input-field source mapping.
    pub arguments: Vec<Argument>,
}

/// A resolved named type reference and structural validation shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueContract {
    /// The exact authored ESS reference.
    pub type_ref: String,
    /// Its supported JSON shape.
    pub shape: Shape,
}

/// One resolved callable interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Callable {
    /// Owner-qualified target.
    pub target: Target,
    /// The declared input struct.
    pub input: Option<ValueContract>,
    /// Success result type.
    pub result: ValueContract,
    /// Declared failure-code payloads.
    pub errors: BTreeMap<String, ValueContract>,
    /// The declared error code that answers every invalid input, with `{}` data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_input: Option<String>,
}

/// Deterministic projection input. Created by the binding compiler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Resolved-plan format.
    pub format: String,
    /// CLI binary name.
    pub binary: String,
    /// CLI description.
    pub about: String,
    /// Global selectors.
    pub globals: Globals,
    /// Resolved callables by stable key.
    pub callables: BTreeMap<String, Callable>,
    /// Sorted command declarations.
    pub commands: Vec<Command>,
    /// Explicit application implementation obligations.
    pub obligations: Vec<String>,
}
