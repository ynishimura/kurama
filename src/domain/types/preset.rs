//! The preset catalog: what kurama knows about a provider's `[auth.*]` and `[api.*]` sections, as const data, and the lookup by id.
//!
//! A preset is material for `kurama preset show` (and `preset add`), never a
//! runtime reference: what it produces is ordinary configuration, fully
//! expanded, so the URLs a secret is sent to are the ones written in the
//! file. A provider with OpenID Connect discovery carries only its `issuer`.
//!
//! A value in braces (`{secret}`, `{client_id}`) in `token`, `client_id`,
//! `client_secret`, the endpoints, `base_url` and the description URL is an input, given
//! with `--set <key>=<value>`; each preset declares exactly the inputs its
//! templates name. The setup steps take their own variables, filled in by
//! `domain::functions::preset_render`: `{docs_url}`, `{redirect_uri}`,
//! `{vault}` and `{item}`.

use super::{DEFAULT_TOKEN_HEADER, GrantType};

/// One entry of the catalog.
#[derive(Debug)]
pub struct Preset {
    pub id: &'static str,
    pub title: &'static str,
    /// Where the credential is created; `preset show --open` opens it.
    pub docs_url: &'static str,
    pub auth: AuthPreset,
    pub api: ApiPreset,
    pub inputs: &'static [Input],
    /// Steps about the API itself (enabling it for a project), shown whether
    /// the auth is added or reused.
    pub setup: &'static [&'static str],
}

/// The `[auth.<name>]` a preset adds, or reuses when a compatible one exists.
#[derive(Debug)]
pub struct AuthPreset {
    /// Kept whatever `--as` says: several presets share one auth.
    pub name: &'static str,
    pub kind: AuthPresetKind,
    pub env_var: Option<&'static str>,
    /// Steps that create the credential; skipped when the auth is reused.
    pub setup: &'static [&'static str],
}

#[derive(Debug)]
pub enum AuthPresetKind {
    OAuth {
        grant_type: GrantType,
        endpoints: PresetEndpoints,
        client_id: &'static str,
        client_secret: Option<&'static str>,
        scopes: &'static [&'static str],
        redirect_port: Option<u16>,
    },
    Token {
        token: &'static str,
        placement: PresetPlacement,
    },
}

/// Where a token preset's credential goes, as the `[auth.*]` keys say it.
#[derive(Debug)]
pub enum PresetPlacement {
    Header {
        /// `None` is kurama's default, `Authorization`.
        header: Option<&'static str>,
        /// `None` is kurama's default, `Bearer {token}`.
        format: Option<&'static str>,
    },
    /// HTTP Basic; `username` is a template (`{email}`).
    Basic {
        username: &'static str,
    },
    Query {
        param: &'static str,
    },
}

/// OpenID Connect discovery, or the URLs of a provider without it.
#[derive(Debug)]
pub enum PresetEndpoints {
    Issuer(&'static str),
    Explicit {
        auth_url: Option<&'static str>,
        token_url: &'static str,
        device_auth_url: Option<&'static str>,
    },
}

/// The one `[api.<name>]` a preset adds; `--as` renames it.
#[derive(Debug)]
pub struct ApiPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub base_url: &'static str,
    /// The API description the section names, under its own key.
    pub spec: Option<PresetSpec>,
    pub headers: &'static [(&'static str, &'static str)],
    /// What follows `kurama api <name>` in the last setup step.
    pub example: &'static str,
}

/// An `[api.*]` description a preset writes: the key and its value.
#[derive(Debug, Clone, Copy)]
pub enum PresetSpec {
    OpenApi(&'static str),
    Discovery(&'static str),
    /// The GraphQL endpoint's path under `base_url`.
    GraphQl(&'static str),
}

impl PresetSpec {
    /// The `[api.*]` key.
    pub fn key(self) -> &'static str {
        match self {
            Self::OpenApi(_) => "openapi",
            Self::Discovery(_) => "discovery",
            Self::GraphQl(_) => "graphql",
        }
    }

    pub fn value(self) -> &'static str {
        match self {
            Self::OpenApi(value) | Self::Discovery(value) | Self::GraphQl(value) => value,
        }
    }

    /// The value when it is a URL of its own; a GraphQL path is under
    /// `base_url`.
    pub fn url(self) -> Option<&'static str> {
        match self {
            Self::OpenApi(url) | Self::Discovery(url) => Some(url),
            Self::GraphQl(_) => None,
        }
    }
}

/// A value the person gives with `--set <key>=<value>`.
#[derive(Debug)]
pub struct Input {
    pub key: &'static str,
    pub help: &'static str,
    /// For a secret: the 1Password field the setup stores it in, so the
    /// suggested reference is `op://<vault>/<item>/<field>`. A secret input
    /// only takes a reference.
    pub secret_field: Option<&'static str>,
}

impl Preset {
    pub fn input(&self, key: &str) -> Option<&Input> {
        self.inputs.iter().find(|input| input.key == key)
    }
}

impl AuthPreset {
    /// The 1Password item the setup creates: no parentheses and no spaces,
    /// which an `op://` reference cannot name.
    pub fn item(&self) -> String {
        format!("kurama-{}", self.name)
    }
}

impl AuthPresetKind {
    /// `token`, or `oauth <grant_type>`, for the list.
    pub fn summary(&self) -> String {
        match self {
            Self::OAuth { grant_type, .. } => format!("oauth {}", grant_type.as_str()),
            Self::Token { .. } => "token".to_owned(),
        }
    }
}

impl PresetPlacement {
    /// The words `TokenPlacement::summary` uses, for the same place.
    pub fn summary(&self) -> String {
        match self {
            Self::Header { header, .. } => header.unwrap_or(DEFAULT_TOKEN_HEADER).to_owned(),
            Self::Basic { .. } => "basic".to_owned(),
            Self::Query { param } => format!("query {param}"),
        }
    }
}

/// The preset `id` names.
pub fn find_preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

const STORE_TOKEN: &str = "Store it in 1Password under a title an op:// reference can name (no parentheses, no spaces):\n\
     op item create --category='API Credential' --vault {vault} --title {item} 'credential=<token>'";

const STORE_CLIENT: &str = "Store the client in 1Password under a title an op:// reference can name (no parentheses, no spaces):\n\
     op item create --category='API Credential' --vault {vault} --title {item} 'client_id=<client-id>' 'client_secret[password]=<client-secret>'";

const SECRET: Input = Input {
    key: "secret",
    help: "reference to the token, such as op://<vault>/<item>/credential",
    secret_field: Some("credential"),
};

const CLIENT_ID: Input = Input {
    key: "client_id",
    help: "the OAuth client ID",
    secret_field: None,
};

const CLIENT_SECRET: Input = Input {
    key: "client_secret",
    help: "reference to the OAuth client secret, such as op://<vault>/<item>/client_secret",
    secret_field: Some("client_secret"),
};

const EMAIL: Input = Input {
    key: "email",
    help: "the email address of the account the token belongs to",
    secret_field: None,
};

const GITHUB_HEADERS: &[(&str, &str)] = &[
    ("Accept", "application/vnd.github+json"),
    ("X-GitHub-Api-Version", "2026-03-10"),
];

/// The description of the REST API version the headers ask for. About 13 MB:
/// it is fetched only by `--ops`, `--describe`, operation targets and the
/// explorer, then cached and revalidated with its ETag.
const GITHUB_OPENAPI: &str = "https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.2026-03-10.json";

const GITHUB_API: ApiPreset = ApiPreset {
    name: "github",
    description: "GitHub REST API",
    base_url: "https://api.github.com",
    spec: Some(PresetSpec::OpenApi(GITHUB_OPENAPI)),
    headers: GITHUB_HEADERS,
    example: "/user",
};

const GOOGLE_CREDENTIALS: &str = "https://console.cloud.google.com/apis/credentials";

/// Shared by the Google presets: the second one reuses the first's
/// `[auth.google]` and warns about the scopes it lacks.
const fn google_auth(scopes: &'static [&'static str]) -> AuthPreset {
    AuthPreset {
        name: "google",
        kind: AuthPresetKind::OAuth {
            grant_type: GrantType::AuthorizationCode,
            endpoints: PresetEndpoints::Issuer("https://accounts.google.com"),
            client_id: "{client_id}",
            client_secret: Some("{client_secret}"),
            scopes,
            redirect_port: Some(8080),
        },
        env_var: None,
        setup: &[
            "Create an OAuth client ID of type Desktop app: {docs_url}\n\
             A Desktop app client accepts the loopback redirect kurama listens on, {redirect_uri}, without registering it.\n\
             A Web application client must register exactly {redirect_uri}",
            STORE_CLIENT,
        ],
    }
}

const fn linear_api() -> ApiPreset {
    ApiPreset {
        name: "linear",
        description: "Linear GraphQL API",
        base_url: "https://api.linear.app",
        spec: Some(PresetSpec::GraphQl("/graphql")),
        headers: &[],
        example: "/graphql -d '{\"query\":\"{ viewer { id name } }\"}'",
    }
}

pub static PRESETS: &[Preset] = &[
    Preset {
        id: "github",
        title: "GitHub REST API with a personal access token",
        docs_url: "https://github.com/settings/personal-access-tokens/new",
        auth: AuthPreset {
            name: "github",
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Header {
                    header: None,
                    format: None,
                },
            },
            env_var: Some("GITHUB_TOKEN"),
            setup: &[
                "Create a fine-grained personal access token with read access to what you will call: {docs_url}",
                STORE_TOKEN,
            ],
        },
        api: GITHUB_API,
        inputs: &[SECRET],
        setup: &[],
    },
    Preset {
        id: "github-oauth",
        title: "GitHub REST API through an OAuth app (device flow)",
        docs_url: "https://github.com/settings/applications/new",
        auth: AuthPreset {
            name: "github",
            kind: AuthPresetKind::OAuth {
                grant_type: GrantType::DeviceCode,
                endpoints: PresetEndpoints::Explicit {
                    auth_url: None,
                    token_url: "https://github.com/login/oauth/access_token",
                    device_auth_url: Some("https://github.com/login/device/code"),
                },
                client_id: "{client_id}",
                client_secret: None,
                scopes: &["read:user", "read:org"],
                redirect_port: None,
            },
            env_var: Some("GITHUB_TOKEN"),
            setup: &[
                "Register an OAuth app and tick Enable Device Flow: {docs_url}\n\
                 The form asks for a homepage and a callback URL; any URL will do, the device flow ignores them and needs no client secret",
            ],
        },
        api: GITHUB_API,
        inputs: &[CLIENT_ID],
        setup: &[],
    },
    Preset {
        id: "google-sheets",
        title: "Google Sheets API v4 (read-only scope)",
        docs_url: GOOGLE_CREDENTIALS,
        auth: google_auth(&["https://www.googleapis.com/auth/spreadsheets.readonly"]),
        api: ApiPreset {
            name: "google-sheets",
            description: "Google Sheets API v4",
            base_url: "https://sheets.googleapis.com",
            spec: Some(PresetSpec::Discovery(
                "https://sheets.googleapis.com/$discovery/rest?version=v4",
            )),
            headers: &[],
            example: "sheets.spreadsheets.get -P spreadsheetId=<spreadsheet-id>",
        },
        inputs: &[CLIENT_ID, CLIENT_SECRET],
        setup: &[
            "Enable the Google Sheets API in the client's project: https://console.cloud.google.com/apis/library/sheets.googleapis.com",
        ],
    },
    Preset {
        id: "google-docs",
        title: "Google Docs API v1 (read-only scope)",
        docs_url: GOOGLE_CREDENTIALS,
        auth: google_auth(&["https://www.googleapis.com/auth/documents.readonly"]),
        api: ApiPreset {
            name: "google-docs",
            description: "Google Docs API v1",
            base_url: "https://docs.googleapis.com",
            spec: Some(PresetSpec::Discovery(
                "https://docs.googleapis.com/$discovery/rest?version=v1",
            )),
            headers: &[],
            example: "docs.documents.get -P documentId=<document-id>",
        },
        inputs: &[CLIENT_ID, CLIENT_SECRET],
        setup: &[
            "Enable the Google Docs API in the client's project: https://console.cloud.google.com/apis/library/docs.googleapis.com",
        ],
    },
    Preset {
        id: "google-drive",
        title: "Google Drive API v3 (read-only scope)",
        docs_url: GOOGLE_CREDENTIALS,
        auth: google_auth(&["https://www.googleapis.com/auth/drive.readonly"]),
        api: ApiPreset {
            name: "google-drive",
            description: "Google Drive API v3",
            base_url: "https://www.googleapis.com/drive/v3",
            spec: Some(PresetSpec::Discovery(
                "https://www.googleapis.com/discovery/v1/apis/drive/v3/rest",
            )),
            headers: &[],
            example: "/files",
        },
        inputs: &[CLIENT_ID, CLIENT_SECRET],
        setup: &[
            "Enable the Google Drive API in the client's project: https://console.cloud.google.com/apis/library/drive.googleapis.com",
        ],
    },
    Preset {
        id: "linear",
        title: "Linear GraphQL API with a personal API key",
        docs_url: "https://linear.app/settings/account/security",
        auth: AuthPreset {
            name: "linear",
            // A personal API key goes in Authorization as it is; Linear
            // refuses it with a Bearer prefix (checked 2026-09-26).
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Header {
                    header: None,
                    format: Some("{token}"),
                },
            },
            env_var: Some("LINEAR_API_KEY"),
            setup: &[
                "Create a personal API key under Security & access: {docs_url}",
                STORE_TOKEN,
            ],
        },
        api: linear_api(),
        inputs: &[SECRET],
        setup: &[],
    },
    Preset {
        id: "linear-oauth",
        title: "Linear GraphQL API through an OAuth application",
        docs_url: "https://linear.app/settings/api/applications/new",
        auth: AuthPreset {
            name: "linear",
            kind: AuthPresetKind::OAuth {
                grant_type: GrantType::AuthorizationCode,
                endpoints: PresetEndpoints::Explicit {
                    auth_url: Some("https://linear.app/oauth/authorize"),
                    token_url: "https://api.linear.app/oauth/token",
                    device_auth_url: None,
                },
                client_id: "{client_id}",
                client_secret: Some("{client_secret}"),
                scopes: &["read"],
                redirect_port: Some(8080),
            },
            env_var: None,
            setup: &[
                "Create an OAuth application: {docs_url}\nAdd this callback URL exactly: {redirect_uri}",
                STORE_CLIENT,
            ],
        },
        api: linear_api(),
        inputs: &[CLIENT_ID, CLIENT_SECRET],
        setup: &[],
    },
    Preset {
        id: "elevenlabs",
        title: "ElevenLabs API with an API key",
        docs_url: "https://elevenlabs.io/app/settings/api-keys",
        auth: AuthPreset {
            name: "elevenlabs",
            // The key goes in xi-api-key as it is, not in Authorization.
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Header {
                    header: Some("xi-api-key"),
                    format: Some("{token}"),
                },
            },
            env_var: Some("ELEVENLABS_API_KEY"),
            setup: &[
                "Create an API key with access to what you will call: {docs_url}",
                STORE_TOKEN,
            ],
        },
        api: ApiPreset {
            name: "elevenlabs",
            description: "ElevenLabs API",
            base_url: "https://api.elevenlabs.io",
            spec: Some(PresetSpec::OpenApi(
                "https://api.elevenlabs.io/openapi.json",
            )),
            headers: &[],
            example: "/v1/models",
        },
        inputs: &[SECRET],
        setup: &[],
    },
    Preset {
        id: "openai",
        title: "OpenAI API with an API key",
        docs_url: "https://platform.openai.com/api-keys",
        auth: AuthPreset {
            name: "openai",
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Header {
                    header: None,
                    format: None,
                },
            },
            env_var: Some("OPENAI_API_KEY"),
            setup: &[
                "Create an API key in the project you will call: {docs_url}",
                STORE_TOKEN,
            ],
        },
        api: ApiPreset {
            name: "openai",
            description: "OpenAI API",
            // The description's paths hang off its `/v1` server.
            base_url: "https://api.openai.com/v1",
            spec: Some(PresetSpec::OpenApi(
                "https://raw.githubusercontent.com/openai/openai-openapi/main/openapi.json",
            )),
            headers: &[],
            example: "/models",
        },
        inputs: &[SECRET],
        setup: &[],
    },
    Preset {
        id: "slack",
        title: "Slack Web API with a bot token",
        docs_url: "https://api.slack.com/apps",
        auth: AuthPreset {
            name: "slack",
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Header {
                    header: None,
                    format: None,
                },
            },
            env_var: Some("SLACK_BOT_TOKEN"),
            setup: &[
                "Create an app, add the bot token scopes you will call and install it to the workspace; copy the Bot User OAuth Token: {docs_url}",
                STORE_TOKEN,
            ],
        },
        api: ApiPreset {
            name: "slack",
            description: "Slack Web API",
            base_url: "https://slack.com/api",
            // The published description was archived in 2021, so none is named.
            spec: None,
            headers: &[],
            example: "/auth.test",
        },
        inputs: &[SECRET],
        setup: &[],
    },
    Preset {
        id: "contentful",
        title: "Contentful Content Management API with a personal access token",
        docs_url: "https://app.contentful.com/account/profile/cma_tokens",
        auth: AuthPreset {
            name: "contentful",
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Header {
                    header: None,
                    format: None,
                },
            },
            env_var: Some("CONTENTFUL_MANAGEMENT_TOKEN"),
            setup: &["Create a personal access token: {docs_url}", STORE_TOKEN],
        },
        api: ApiPreset {
            name: "contentful",
            description: "Contentful Content Management API",
            base_url: "https://api.contentful.com",
            spec: None,
            headers: &[],
            example: "/users/me",
        },
        inputs: &[SECRET],
        setup: &[],
    },
    Preset {
        id: "fireworks",
        title: "Fireworks AI inference API with an API key",
        docs_url: "https://app.fireworks.ai/settings/users/api-keys",
        auth: AuthPreset {
            name: "fireworks",
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Header {
                    header: None,
                    format: None,
                },
            },
            env_var: Some("FIREWORKS_API_KEY"),
            setup: &["Create an API key: {docs_url}", STORE_TOKEN],
        },
        api: ApiPreset {
            name: "fireworks",
            description: "Fireworks AI inference API",
            base_url: "https://api.fireworks.ai/inference/v1",
            spec: None,
            headers: &[],
            example: "/models",
        },
        inputs: &[SECRET],
        setup: &[],
    },
    Preset {
        id: "jira",
        title: "Jira Cloud REST API with an API token",
        docs_url: "https://id.atlassian.com/manage-profile/security/api-tokens",
        auth: AuthPreset {
            name: "jira",
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Basic {
                    username: "{email}",
                },
            },
            env_var: Some("JIRA_API_TOKEN"),
            setup: &[
                "Create an API token for the account whose email you give: {docs_url}",
                STORE_TOKEN,
            ],
        },
        api: ApiPreset {
            name: "jira",
            description: "Jira Cloud REST API",
            base_url: "https://{site}.atlassian.net",
            spec: Some(PresetSpec::OpenApi(
                "https://developer.atlassian.com/cloud/jira/platform/swagger-v3.v3.json",
            )),
            headers: &[],
            example: "/rest/api/3/myself",
        },
        inputs: &[
            SECRET,
            Input {
                key: "site",
                help: "the site name, as in <site>.atlassian.net",
                secret_field: None,
            },
            EMAIL,
        ],
        setup: &[],
    },
    Preset {
        id: "zendesk",
        title: "Zendesk Support API with an API token",
        docs_url: "https://support.zendesk.com/hc/en-us/articles/4408889192858",
        auth: AuthPreset {
            name: "zendesk",
            // Zendesk reads `<email>/token` as the username of an API token.
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Basic {
                    username: "{email}/token",
                },
            },
            env_var: Some("ZENDESK_API_TOKEN"),
            setup: &[
                "Enable token access and add an API token in the Admin Center: {docs_url}",
                STORE_TOKEN,
            ],
        },
        api: ApiPreset {
            name: "zendesk",
            description: "Zendesk Support API",
            base_url: "https://{subdomain}.zendesk.com",
            spec: Some(PresetSpec::OpenApi(
                "https://developer.zendesk.com/zendesk/oas.yaml",
            )),
            headers: &[],
            example: "/api/v2/users/me",
        },
        inputs: &[
            SECRET,
            Input {
                key: "subdomain",
                help: "the account's subdomain, as in <subdomain>.zendesk.com",
                secret_field: None,
            },
            EMAIL,
        ],
        setup: &[],
    },
    Preset {
        id: "backlog",
        title: "Backlog API with an API key",
        docs_url: "https://support.nulab.com/hc/en-us/articles/8591094281113",
        auth: AuthPreset {
            name: "backlog",
            kind: AuthPresetKind::Token {
                token: "{secret}",
                placement: PresetPlacement::Query { param: "apiKey" },
            },
            env_var: Some("BACKLOG_API_KEY"),
            setup: &[
                "Register an API key under Personal Settings > API in your space: {docs_url}",
                STORE_TOKEN,
            ],
        },
        api: ApiPreset {
            name: "backlog",
            description: "Backlog API",
            base_url: "https://{host}/api/v2",
            spec: None,
            headers: &[],
            example: "/users/myself",
        },
        inputs: &[
            SECRET,
            Input {
                key: "host",
                help: "the space's host, such as example.backlog.com or example.backlog.jp",
                secret_field: None,
            },
        ],
        setup: &[],
    },
];
