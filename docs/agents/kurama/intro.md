# kurama for agents and scripts

kurama turns a credential source into credentials: an AWS profile in
`~/.aws/config` into temporary role credentials (STS AssumeRole, MFA codes
from 1Password, MFA sessions cached in the macOS keychain) -- or, for a
profile without `role_arn`, into the IAM user's own long-term keys or the MFA
session they get -- or an `[auth.*]`
source in `~/.config/kurama/config.toml` into a token -- from an OAuth grant
(`kind = "oauth"`, stored in the keychain) or from the secret store an API
key already lives in (`kind = "token"`, stored nowhere). `kurama api` calls
an `[api.*]` profile with that credential, or with a SigV4 signature from an
AWS profile's role credentials (`aws_profile`). This page is the contract for coding agents and scripts:
`kurama agent` prints it, and `kurama agent --skill` prints an Agent Skill
(`SKILL.md`) that tells an agent to read it first.

