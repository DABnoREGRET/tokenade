//! **What we install, and where.**
//!
//! The installation's types: a site (the agent, its configuration file, its
//! flavour), a recipe, a plan, and the record of what was written. Nothing here
//! touches the disk — it is the vocabulary the rest of the module speaks.

use super::*;

/// Marks the provider entry this product owns, in every format.
///
/// A name rather than a comment: config formats lose comments through some
/// editors, and this has to survive a round trip through the agent's own
/// writer. It is also what makes a second install recognisable as a re-install.
pub const PROVIDER_ID: &str = "tokenade";

/// Why an install could not proceed.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The agent's config file is not where we expect it, and creating one from
    /// nothing would mean inventing the rest of that agent's configuration.
    NoConfig(PathBuf),
    /// The file exists and is not parseable. We do not rewrite what we could
    /// not read: a proxy install is not worth corrupting somebody's setup.
    Unparseable(PathBuf),
    /// We would have captured our own address as the upstream.
    WouldLoop(String),
    /// The address we would write into the agent's config carries a PATH.
    ///
    /// The proxy composes `upstream + <what the agent asked for>`, so any path
    /// on our own base URL is prepended to every request and then joined to an
    /// upstream that already has one: `https://api.openai.com/v1/v1/responses`.
    /// A fake provider that answers 200 to anything hides this entirely, and a
    /// real one answers 404 on every turn.
    ListenUrlHasPath(String),
    /// No recipe: nothing has been measured for this agent, and guessing a
    /// config key is how a product claims a coverage it never verified.
    NoRecipeMeasured(&'static str),
    /// The key path is measured, the config is readable, and it names a
    /// provider whose base URL is neither written down nor known here.
    ///
    /// Distinct from [`Self::NoRecipeMeasured`] on purpose: there the SITE is
    /// unknown, here the site is known and the UPSTREAM is not. Every other
    /// recipe can fall back to one vendor default because the config file
    /// belongs to one vendor; cline's does not — it multiplexes providers, so
    /// "no base URL written" means "whatever that provider's default is", and
    /// this module has no business inventing it.
    NoUpstreamMeasured {
        agent: &'static str,
        provider: String,
    },
    Io(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // "no file" and "a file without the block it needs" call for the
            // same action, and the message says so without lying about which
            // of the two it is.
            Self::NoConfig(p) => write!(
                f,
                "no usable config at {} — run the agent once so it writes one \
                 (a config that names no provider is not enough), then install again",
                p.display()
            ),
            Self::Unparseable(p) => write!(
                f,
                "{} could not be parsed, so it was left untouched",
                p.display()
            ),
            Self::WouldLoop(u) => write!(
                f,
                "refusing to install: the agent already points at {u}, which is this proxy. \
                 Uninstall first, or the proxy would become its own upstream."
            ),
            Self::NoRecipeMeasured(a) => write!(
                f,
                "no measured config site for {a} — nobody has checked where it keeps its \
                 base URL, and guessing one would be a claim rather than a measurement"
            ),
            Self::NoUpstreamMeasured { agent, provider } => write!(
                f,
                "{agent}'s config selects the provider {provider:?} and writes no base URL for \
                 it, so what it talks to today is that provider's own default and this tool \
                 does not know it. Set the base URL explicitly once (`cline auth --provider \
                 {provider} --baseurl <url>`), then install."
            ),
            Self::ListenUrlHasPath(u) => write!(
                f,
                "the proxy address written into an agent's config must carry no path, got {u:?}. \
                 The agent appends its own route and the proxy appends that to the provider's \
                 base URL, so a path here is sent twice."
            ),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// What an install will do, computed before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub agent: Agent,
    pub path: PathBuf,
    /// True when the agent's config named a base URL; false when `upstream` is
    /// the vendor default this recipe assumed.
    pub had_explicit_upstream: bool,
    /// What the agent talks to TODAY. This becomes the proxy's upstream, and it
    /// is the field the whole read-then-replace dance exists to obtain.
    pub upstream: String,
    /// The provider entry the agent was selecting, so uninstall can put the
    /// selector back rather than leave the agent on a `tokenade` entry that no
    /// longer resolves.
    pub previous_selector: Option<String>,
    /// Where the agent will be pointed.
    pub listen_url: String,
    /// True when the config file does not exist yet and this recipe is allowed
    /// to create it. Only a dotenv site is, and only when the agent's own
    /// directory is already there — the directory is the proof the agent is
    /// installed, and without it we would be scattering files for agents the
    /// machine does not have.
    pub created_file: bool,
}

/// The record kept so an uninstall does not have to guess.
///
/// Written BEFORE the config is touched. If the process dies between the two,
/// the worst case is a record for an install that did not happen, which
/// uninstall treats as a no-op; the opposite order would leave a rewritten
/// config with nothing that remembers what it used to say.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    pub agent: String,
    pub path: PathBuf,
    pub upstream: String,
    pub previous_selector: Option<String>,
    /// Whether `upstream` was read from the agent's config or is the vendor
    /// default we assumed. Uninstall must not write an assumption back as if
    /// it were the user's setting.
    #[serde(default)]
    pub had_explicit_upstream: bool,
    /// Whether this install CREATED the config file. Uninstall removes a file
    /// it created and left empty, so the machine comes back byte-for-byte.
    #[serde(default)]
    pub created_file: bool,
    /// The `host:port` this agent was pointed at.
    ///
    /// One process serves every agent, and the PORT is what tells it which
    /// agent a connection belongs to — which decides the upstream and whether
    /// folding is allowed at all. Recorded here so `serve` can rebuild the
    /// whole set from the records alone, with nothing typed twice.
    ///
    /// `#[serde(default)]` because records written before this field existed
    /// are still readable; an empty string means "the default port", which is
    /// what a single-agent install used.
    #[serde(default)]
    pub listen: String,
}

/// Which agents have a MEASURED config site.
///
/// Deliberately short. `agent_matrix` reaches fourteen agents by environment
/// variable, and none of those count here: the constraint is config files, and
/// the exact key path for an agent is a measurement, not an inference from the
/// fact that a bundle contains the string `baseUrl` somewhere.
pub fn recipe(agent: Agent) -> Option<Site> {
    match agent {
        Agent::Codex => Some(Site::CodexToml),
        Agent::Cursor => Some(Site::ShellProfile {
            var: "CURSOR_API_ENDPOINT",
        }),
        Agent::Aider => Some(Site::AiderYaml),
        // Same file schema, and not by resemblance: both configs on this
        // machine declare `"$schema": "https://opencode.ai/config.json"`, kilo
        // being an opencode fork. The shared shape is measured in both bundles
        // as `options:{baseURL`.
        Agent::OpenCode => Some(Site::OpenCodeJson {
            path: OpenCodeFlavour::OpenCode,
        }),
        Agent::KiloCode => Some(Site::OpenCodeJson {
            path: OpenCodeFlavour::Kilo,
        }),
        // `~/.claude/settings.json` carries an `env` block that Claude Code
        // applies to its own process. That is a FILE — one we can back up, diff
        // and put back — and not a shell rc, so it is inside the operator's
        // constraint rather than a way around it. Measured 2026-08-16: the key
        // is already in use on this machine
        // (`env.CLAUDE_CODE_AUTO_COMPACT_WINDOW`).
        //
        // Installing here is for the COST FIGURES only: the matrix keeps
        // Claude Code at `AccountOnlyHigherRungOwns`, so the proxy relays its
        // payloads untouched while reading the provider's own usage.
        Agent::ClaudeCode => Some(Site::SettingsEnvJson {
            flavour: SettingsFlavour::ClaudeCode,
        }),
        // Same mechanism, measured on qwen's own bundle rather than assumed
        // from Claude's: `settings.env` ×40 and `OPENAI_BASE_URL` ×21. Qwen is
        // a Gemini CLI fork that talks the OpenAI wire through DashScope.
        Agent::Qwen => Some(Site::SettingsEnvJson {
            flavour: SettingsFlavour::Qwen,
        }),

        // Measured 2026-08-17 in grok's OWN shipped documentation
        // (`~/.grok/docs/user-guide/11-custom-models.md`), which is a stronger
        // source than a string in a bundle: "Config File Alternative —
        // `[endpoints] models_base_url`. When you use `[endpoints]` with
        // partial model overrides, Grok inherits the `base_url` from the
        // endpoints config." The vendor default is in the binary itself.
        Agent::Grok => Some(Site::GrokToml),

        // Measured 2026-08-17 by RUNNING the agent's own writer:
        // `cline auth --data-dir <tmp> --provider openai-native --apikey … \
        //  --baseurl http://127.0.0.1:9999/v1` produced
        // `settings/providers.json` carrying exactly
        // `providers.openai-native.settings.baseUrl`. Running the writer beats
        // reading the reader: it answers the key path AND the file location in
        // one measurement, and it cannot be wrong about the shape.
        Agent::Cline => Some(Site::ClineProvidersJson),

        // The two agents this table classed as "env var only, and we do not
        // touch shell rc files" and that turn out to have a FILE.
        //
        // The general lesson, and it applies to the four rows still marked
        // env-only: "reachable by an environment variable" is a statement
        // about the AGENT, not about how the user must set it. Claude Code
        // was always in that class and always had a recipe, because its
        // settings file carries an `env` block. Asking each agent's own
        // writer whether it accepts one is a thirty-second test, and it was
        // never run until 2026-08-19.
        Agent::OpenClaw => Some(Site::SettingsEnvJson {
            flavour: SettingsFlavour::OpenClaw,
        }),
        Agent::Copilot => Some(Site::SettingsEnvJson {
            flavour: SettingsFlavour::Copilot,
        }),
        Agent::Hermes => Some(Site::HermesYaml),
        Agent::Droid => Some(Site::FactoryConfigJson),
        Agent::Pi => Some(Site::PiModelsStore),
        Agent::CopilotVsCode => Some(Site::VsCodeCopilotSettings),
        Agent::DevinDesktop => Some(Site::VsCodeSettingsJson),

        _ => None,
    }
}

/// The site of an agent REMOVED from the product (Gemini CLI, Reasonix), or of
/// a record under an old name (`windsurf`, now `devin-desktop`).
///
/// No more recipe: we no longer install anything for them. But an older
/// install left a record, and `uninstall` must still know how to put the file
/// back the way it was.
pub fn legacy_site(slug: &str) -> Option<Site> {
    match slug {
        "gemini-cli" => Some(Site::GeminiEnvFile),
        "windsurf" => Some(Site::VsCodeSettingsJson),
        "reasonix" => Some(Site::ReasonixToml),
        _ => None,
    }
}

/// Which of the two opencode-schema agents a site belongs to.
///
/// The schema is shared and the FILE is not, so this is the only thing that
/// differs between the two recipes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeFlavour {
    OpenCode,
    Kilo,
}

/// How one agent's config is shaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// `~/.codex/config.toml`: a `model_provider` selector plus a
    /// `[model_providers.<id>]` table carrying `base_url` and `wire_api`.
    ///
    /// Measured 2026-08-15 in the binary the npm package fetches
    /// (`@openai/codex-linux-x64/vendor/.../bin/codex`): `base_url` ×74,
    /// `wire_api` ×22, `env_key` ×34, and the only route string in the whole
    /// package is `v1/responses`.
    CodexToml,
    /// `~/.aider.conf.yml`: flat, one key per line, every long CLI flag usable
    /// as a key. Measured: `args.py` searches for `.aider.conf.yml` in the git
    /// root, the cwd and the home directory, and `--openai-api-base` is a
    /// declared flag.
    AiderYaml,
    /// The opencode config schema: `provider.<id>.options.baseURL`, with the
    /// provider id taken from the `model` key (`"<provider>/<model>"`).
    ///
    /// Written through a `serde_json` round trip, which sorts keys — the same
    /// thing this product already does to these very files when it installs an
    /// MCP server (`install/mcp_writers.rs`). Codex gets `toml_edit` instead
    /// because its file carries hook `trusted_hash` entries that a reordering
    /// writer would be much more dangerous around; here the cost is cosmetic
    /// and inventing a third writing strategy would be worse.
    OpenCodeJson { path: OpenCodeFlavour },
    /// A `settings.json` with an `env` block the agent applies to its own
    /// process. A FILE, so it is inside the config-only constraint rather than
    /// a way around it — we can back it up, diff it and put it back, none of
    /// which is true of a shell rc.
    SettingsEnvJson { flavour: SettingsFlavour },
    /// The login shell's profile, for an agent whose ONLY route is an
    /// environment variable.
    ///
    /// Forbidden until 2026-08-21 by an operator constraint — "an agent we
    /// ship an installer for must be reachable through a file we can back up
    /// and put back" — lifted by them that day. The constraint is in fact
    /// respected to the letter: a shell profile IS a file, it is backed up like
    /// the others, and the written block is delimited by markers, so uninstall
    /// removes it byte for byte.
    ///
    /// The only agent concerned is cursor: measured 2026-08-19 then re-measured
    /// on 08-21, `CURSOR_API_ENDPOINT` is its only route — a `.cursor/cli.json`
    /// carrying `endpoint`/`apiUrl` makes the CLI exit 1 with
    /// `Unrecognized key(s) in object`.
    ShellProfile { var: &'static str },
    /// `~/.grok/config.toml`: `[endpoints] models_base_url`.
    ///
    /// A flat key rather than a provider table, and that is the whole recipe —
    /// grok inherits this base URL into every `[model.*]` section that does not
    /// override it, so one key redirects the agent. `toml_edit` for the same
    /// reason as Codex: the file carries the user's marketplace sources, their
    /// permission mode and their UI settings.
    GrokToml,
    /// `~/.reasonix/config.toml`: `base_url` on the `[[providers]]` entry the
    /// `default_model` key selects by name.
    ///
    /// An array of tables rather than a keyed one, so the selector is a FIELD
    /// of the entry instead of its key — the same trap as cline's
    /// `lastUsedProvider`, reached differently. Measured 2026-08-20: with
    /// `base_url` pointed at a listener here, `reasonix -p` sent it `POST
    /// /v1/messages`, which is why this row's wire is Anthropic and not the
    /// `kind = "anthropic"` label in the file.
    ReasonixToml,
    /// `~/.hermes/config.yaml`: `model.base_url`, nested one level under the
    /// `model:` block.
    ///
    /// This agent was classed "environment variable only" and it is not:
    /// measured 2026-08-19 on the config this machine's hermes actually
    /// carries, line 4 reads `  base_url: https://openrouter.ai/api/v1`, and
    /// `hermes config set` is its own writer for that file. Line-based like
    /// the aider recipe, and scoped to the `model:` block so a `base_url`
    /// belonging to some other section is never touched.
    HermesYaml,
    /// `~/.cline/settings/providers.json`:
    /// `providers.<lastUsedProvider>.settings.baseUrl`.
    ///
    /// The selector is a sibling key rather than a slash-separated model name,
    /// which is the only structural difference from the opencode schema. It
    /// matters for the same reason: writing under the wrong provider leaves the
    /// agent talking to its vendor with nothing looking wrong.
    ClineProvidersJson,
    /// `~/.gemini/.env`: `CODE_ASSIST_ENDPOINT`.
    ///
    /// Measured 2026-08-19 end to end, not read off a bundle: with
    /// `CODE_ASSIST_ENDPOINT` pointing at a listener on this machine, gemini
    /// sent it `POST /v1internal:loadCodeAssist` and `POST
    /// /v1internal:onboardUser`, through the ENV VAR and again through
    /// `~/.gemini/.env`, which gemini loads itself. The matrix carried
    /// `GOOGLE_GEMINI_BASE_URL` for this agent and that string does not appear
    /// in the shipped bundle at all.
    ///
    /// A dotenv file is a FILE, so this stays inside the config-only
    /// constraint the same way the `settings.json` env blocks do.
    GeminiEnvFile,
    /// `~/.factory/config.json`: `custom_models[].base_url`.
    ///
    /// Measured 2026-08-19: droid sent `POST /v1/chat/completions` to a
    /// listener on this machine named by a `custom_models` entry. `base_url`
    /// is one of the two fields droid's own filter requires of an entry
    /// (`typeof n.base_url === "string" || bedrock`), so this is the field it
    /// dials, not a field it stores.
    ///
    /// Only a model the user already configured is redirected. droid's own
    /// models go to Factory's service, and pointing THOSE at the proxy needs
    /// `FACTORY_API_BASE_URL` — an environment variable, with no file droid
    /// reads it from (`~/.factory/.env` was tried and ignored).
    FactoryConfigJson,
    /// `~/.pi/agent/models-store.json`: `<provider>.models[].baseUrl`.
    ///
    /// Measured 2026-08-19: with the anthropic models' `baseUrl` pointed at a
    /// listener here, pi sent it `POST /v1/messages` — the Anthropic wire this
    /// proxy rewrites, so pi is a fold-and-account agent and not an
    /// account-only one. The provider comes from `settings.json` next door,
    /// and every model under it is redirected, because pi cycles models within
    /// a provider and redirecting one would leave the others talking to the
    /// vendor with nothing looking wrong.
    PiModelsStore,
    /// Devin Desktop's VS Code settings: `codeium.inferenceApiServerUrl`.
    ///
    /// Measured 2026-08-19 by running the thing that dials, not the app: the
    /// extension pushes this setting to `language_server_linux_x64` as
    /// `--inference_api_server_url`, and the binary run with that flag pointed
    /// at a listener here sent it
    /// `POST /exa.api_server_pb.ApiServerService/Ping` and
    /// `GET /exa.api_server_pb.ApiServerService/GetCompletions`.
    ///
    /// Installing here buys reach and nothing else, and the matrix says so:
    /// the wire is Connect/protobuf, so a body is relayed untouched AND the
    /// usage numbers are not readable either — they live in JSON this
    /// conversation never carries.
    VsCodeSettingsJson,
    /// VS Code's own `settings.json`, holding Copilot's CAPI override.
    ///
    /// A different file AND a different key from [`Self::VsCodeSettingsJson`],
    /// which is Devin's fork. Same flat dotted-key shape, so it reuses the
    /// same three helpers.
    VsCodeCopilotSettings,
}

/// Which agent's settings file, and therefore which key and which default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsFlavour {
    ClaudeCode,
    Qwen,
    /// OpenClaw. Its config is validated by a CLOSED schema, so this is not a
    /// guess: measured 2026-08-19 with the agent's own writer, `openclaw config
    /// set`, which answers `Unrecognized key` on anything it does not accept.
    /// Four candidate paths for a base URL were REFUSED —
    /// `auth.profiles.<id>.baseUrl`, `providers.<id>.baseUrl`,
    /// `agents.defaults.baseUrl`, `models.<id>.baseUrl` — and a top-level
    /// `env` block was ACCEPTED, with openclaw answering "Updated
    /// env.OPENAI_BASE_URL. Restart the gateway to apply." That is the same
    /// shape Claude Code and Qwen already use: a FILE, so it stays inside the
    /// config-only constraint.
    OpenClaw,
    /// GitHub Copilot CLI. Its `~/.copilot/config.json` says in a comment
    /// "User settings belong in settings.json", and measured 2026-08-19: a
    /// `settings.json` carrying an `env` block is READ and RE-SERIALISED by
    /// copilot (the file came back compacted, key intact), which is what a
    /// config it ignores would never do.
    Copilot,
}

impl SettingsFlavour {
    /// The environment name this agent reads its base URL from.
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::ClaudeCode => "ANTHROPIC_BASE_URL",
            Self::Qwen => "OPENAI_BASE_URL",
            // OpenRouter is OpenAI-compatible and openclaw embeds `@cline/llms`
            // to talk to it; `OPENAI_BASE_URL` is present in its bundle and
            // accepted by its schema.
            Self::OpenClaw => "OPENAI_BASE_URL",
            // The agent's OWN namespaced variable, preferred over the two
            // generic ones its bundle also carries: it can only mean this
            // product, so it cannot collide with another tool's override.
            Self::Copilot => "COPILOT_PROVIDER_BASE_URL",
        }
    }
}

impl Site {
    /// The config file this site lives in, under `home`.
    pub fn path(self, home: &Path) -> PathBuf {
        match self {
            Site::ShellProfile { .. } => shell_profile_path(home),
            Site::CodexToml => home.join(".codex").join("config.toml"),
            Site::AiderYaml => home.join(".aider.conf.yml"),
            Site::OpenCodeJson {
                path: OpenCodeFlavour::OpenCode,
            } => home.join(".config").join("opencode").join("opencode.json"),
            Site::OpenCodeJson {
                path: OpenCodeFlavour::Kilo,
            } => home.join(".config").join("kilo").join("kilo.json"),
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::ClaudeCode,
            } => home.join(".claude").join("settings.json"),
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::Qwen,
            } => home.join(".qwen").join("settings.json"),
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::OpenClaw,
            } => home.join(".openclaw").join("openclaw.json"),
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::Copilot,
            } => home.join(".copilot").join("settings.json"),
            Site::GrokToml => home.join(".grok").join("config.toml"),
            Site::ReasonixToml => home.join(".reasonix").join("config.toml"),
            Site::HermesYaml => home.join(".hermes").join("config.yaml"),
            Site::ClineProvidersJson => home.join(".cline").join("settings").join("providers.json"),
            Site::GeminiEnvFile => home.join(".gemini").join(".env"),
            Site::FactoryConfigJson => home.join(".factory").join("config.json"),
            Site::PiModelsStore => home.join(".pi").join("agent").join("models-store.json"),
            // The per-user config root differs per OS, the way every VS Code
            // derivative's does. Same three roots `mcp_wrap` already resolves
            // for Claude Desktop.
            Site::VsCodeSettingsJson => {
                let root = if cfg!(target_os = "macos") {
                    home.join("Library").join("Application Support")
                } else if cfg!(target_os = "windows") {
                    home.join("AppData").join("Roaming")
                } else {
                    home.join(".config")
                };
                root.join("Devin").join("User").join("settings.json")
            }
            Site::VsCodeCopilotSettings => {
                let root = if cfg!(target_os = "macos") {
                    home.join("Library").join("Application Support")
                } else if cfg!(target_os = "windows") {
                    home.join("AppData").join("Roaming")
                } else {
                    home.join(".config")
                };
                root.join("Code").join("User").join("settings.json")
            }
        }
    }

    /// What this agent talks to when its config says nothing.
    ///
    /// Needed because "no base URL configured" is the COMMON case — an agent
    /// using its vendor's default — and it is exactly the case where the proxy
    /// is most useful. Refusing to install without an explicit value would
    /// limit the rung to users who had already customised their provider.
    pub(super) fn vendor_default(self) -> &'static str {
        match self {
            Site::ShellProfile { .. } => CURSOR_DEFAULT_UPSTREAM,
            Site::CodexToml | Site::AiderYaml | Site::OpenCodeJson { .. } => {
                "https://api.openai.com/v1"
            }
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::ClaudeCode,
            } => "https://api.anthropic.com",
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::Qwen,
            } => "https://dashscope.aliyuncs.com/compatible-mode/v1",
            // Read off the config this machine's openclaw actually carries
            // (`model.primary` is an `openrouter/…` id and its auth profile's
            // provider is `openrouter`), not off a vendor page.
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::OpenClaw,
            } => "https://openrouter.ai/api/v1",
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::Copilot,
            } => "https://api.githubcopilot.com",
            // Read out of the grok binary itself rather than out of a blog
            // post: `https://api.x.ai/v1` is the only xAI API base string in
            // `~/.grok/bin/grok`.
            Site::GrokToml => "https://api.x.ai/v1",
            Site::ReasonixToml => "https://api.deepseek.com",
            // Read off this machine's own hermes config, not off a vendor page.
            Site::HermesYaml => "https://openrouter.ai/api/v1",
            // Never reached: cline is the one site whose file does not belong
            // to a single vendor, so `plan` refuses instead of defaulting.
            // Present because the enum is exhaustive, and it names the reason
            // rather than picking a URL nobody measured.
            Site::ClineProvidersJson => UPSTREAM_MUST_BE_EXPLICIT,
            // Read out of the shipped bundle's own constant, not a docs page:
            // `CODE_ASSIST_ENDPOINT = "https://cloudcode-pa.googleapis.com"`.
            Site::GeminiEnvFile => "https://cloudcode-pa.googleapis.com",
            Site::FactoryConfigJson | Site::PiModelsStore => UPSTREAM_MUST_BE_EXPLICIT,
            // The extension's own default, read where it builds the flag.
            Site::VsCodeSettingsJson => "https://inference.codeium.com",
            Site::VsCodeCopilotSettings => "https://api.githubcopilot.com",
        }
    }
}
