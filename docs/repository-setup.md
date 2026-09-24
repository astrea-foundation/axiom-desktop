# Repository and release credentials

Local builds and deterministic CI do not require production signing credentials.
The signed [release workflow](../.github/workflows/release.yml) uses the `release`
environment. [Windows signing](../.github/workflows/windows-signed.yml) uses
`windows-signing` and Azure OpenID Connect (OIDC).

Git source does not include GitHub repository settings. A new repository needs
its own environments, variables, secrets, branch/tag protections and external
trust configuration. Keep development on `dev`; production publication requires
explicit promotion to `main` under [CONTRIBUTING](../CONTRIBUTING.md).

## Release environment secrets

In the new repository, open **Settings → Environments → release** and add these
environment secrets. The names below match the workflow exactly.

| Secret | Value and reuse |
|---|---|
| `MACOS_CERTIFICATE_BASE64` | Base64 of the password-protected PKCS#12 bundle containing both Developer ID Application and Developer ID Installer identities, including their private keys. Reuse valid existing identities; `.cer` files alone are insufficient. |
| `MACOS_CERTIFICATE_PASSWORD` | The password used to export that PKCS#12 bundle. |
| `APPLE_ID` | Apple account authorized to notarize for the developer team. |
| `APPLE_APP_SPECIFIC_PASSWORD` | App-specific password for that Apple account. Reuse a valid retained value or generate a replacement if it is unavailable. |
| `APPLE_TEAM_ID` | The developer team ID matching the signing identities. |
| `AXIOM_UPDATE_SIGNING_KEY` | Existing Ed25519 update-signing private key in PKCS#8 PEM format. Preserve its relationship to the public key trusted by installed clients and the platform publisher. |
| `AXIOM_EVAL_API_KEY` | An Axiom automation API key for the funded release evaluation account. It is not a NEAR or Tinfoil provider key. |
| `AXIOM_EVAL_BASE_URL` | Relay HTTPS base URL used by the live release gate. The current workflow reads it as a secret even though the URL is not confidential. |
| `AXIOM_EVAL_MODEL` | Available model ID for the live attested E2EE gate. The current workflow reads it as a secret even though the model ID is not confidential. |
| `AXIOM_RELEASES_TOKEN` | Fine-grained GitHub credential with Contents write on the public installer distribution repository. Needed for automated mirror publication. |
| `AXIOM_PLATFORM_RELEASE_TOKEN` | Fine-grained GitHub credential with Actions write on the private platform repository. Needed to dispatch its publisher workflow. |

The first nine entries are required for the current signed release pipeline.
The final two enable automatic cross-repository publication; without them, the
workflow retains signed artifacts for the documented operator handoff.

Existing certificates and keys do not need replacement merely because a repository
changes. Re-enter their retained values in the new environment. GitHub's secret
API returns metadata, not stored secret values; use the original secure backups.
For shared organization secrets, grant the new repository access rather than
duplicating values. Do not expose secrets in logs to recover them. See
[GitHub secret management](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets)
and [the secret API](https://docs.github.com/en/rest/actions/secrets).

If the update private key is unavailable, stop and plan trust-key rotation before
publishing. Generating an unrelated key does not make existing installations trust
it. The overlap procedure is in [release trust setup](releasing.md#update-trust-and-release-setup).

`GITHUB_TOKEN` is supplied automatically for the workflow's own repository. No
manual copy is needed. Provider keys, database credentials, Cloudflare credentials,
payment keys and production SSH credentials remain in the platform infrastructure.

## Variables

Create **repository variable** `AXIOM_UPDATE_PUBLIC_KEYS` under **Settings →
Secrets and variables → Actions → Variables**. Its value is the comma-separated,
lowercase 32-byte Ed25519 public keys in hex. Keep it consistent with the existing
signer and platform publisher. This must be available at repository scope because
some native build jobs do not select a signing environment.

Create the following **environment variables** under **Settings → Environments →
windows-signing**:

| Variable | Value |
|---|---|
| `AZURE_CLIENT_ID` | Client ID of the existing user-assigned managed identity used for signing |
| `AZURE_TENANT_ID` | Existing Entra tenant ID |
| `AZURE_SUBSCRIPTION_ID` | Subscription containing the signing service |
| `AXIOM_SIGNING_ENDPOINT` | Azure signing endpoint |
| `AXIOM_SIGNING_ACCOUNT` | Signing account name |
| `AXIOM_SIGNING_PROFILE` | Certificate profile name |
| `AXIOM_SIGNING_PUBLISHER` | Expected publisher identity checked on signed binaries |

These are configuration values, not private signing keys. The Windows workflow
does not use an Azure client-secret password.

## External trust and publication

Add an Azure federated credential to the existing user-assigned managed identity
for the repository's signing environment. This identity is managed under Azure
**Managed Identities**, not **App registrations**.
Match its actual OIDC subject format. GitHub documents an immutable default for
repositories created after July 15, 2026, including owner and repository IDs:

```text
issuer:   https://token.actions.githubusercontent.com
subject:  repo:OWNER@OWNER_ID/NEW_REPOSITORY@REPOSITORY_ID:environment:windows-signing
audience: api://AzureADTokenExchange
```

Repositories using the name-only subject format instead use
`repo:OWNER/NEW_REPOSITORY:environment:windows-signing`. Do not copy that format
from an existing repository without checking the new repository's OIDC settings.
Preserve the existing Azure signing permissions and review the environment's
branch/tag restrictions and required reviewers. A custom OIDC subject policy
must use its configured claim format instead. See the
[OIDC subject reference](https://docs.github.com/en/actions/reference/security/oidc#immutable-subject-claims)
and
[GitHub's Azure OIDC guide](https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-in-azure).

Before the first release from a new repository:

1. Recreate environment protection, stable-tag restrictions, branch rules and
   maintainer/Actions access. Secrets should be available only to reviewed release
   code. Fork contribution tests do not need release credentials.
2. Keep source-repository references aligned in
   [the native publisher](../scripts/publish-github-release.mjs) and the platform
   publisher. Both must name `astrea-foundation/axiom-desktop`; the public installer
   mirror remains `astrea-foundation/axiom-releases`. Configure the private platform
   publisher before the first release; copying native source does not update it.
3. Update the platform's `AXIOM_NATIVE_READ_TOKEN` repository access where required,
   plus its configured native repository identity and tag-ancestry checks. Keep
   platform credentials in that private repository. Public source can permit
   anonymous reads, but the current publisher must be adapted before removing a
   credential it still requires.
4. Run CI against the new source revision and publish a new version. Manifests
   bind the exact commit hash, and publication checks verify source-tag ancestry.
   Do not change existing signed inventories or relabel released installer bytes
   as builds from the new source history.

Credential names above are the checked-in workflow contract, not confirmation
that a particular GitHub repository or Azure tenant is configured.
