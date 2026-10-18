# Agent of Empires OMP integration

This directory contains two separately installed components: the AoE plugin declared by `aoe-plugin.toml` and the OMP runtime extension under `omp-extension/`.

The host requires AoE plugin API 18. Installing the integration never approves prompts automatically.

## Runtime compatibility

The independently versioned integration is **0.2.0** in both `aoe-plugin.toml` and `omp-extension/package.json`. This is not an OMP version. `api_version = 18` describes the AoE host API, optional `aoe_version` restricts the AoE application, and report/reply `schema_version = 1` describes the provider wire protocol.

Supported OMP runtimes are exactly **18.10.3**, **18.10.4**, and **18.11.1**, not all later releases. Both manifests declare this set. The extension checks the running host's injected `pi.pi.VERSION` before registering handlers; unsupported or unknown versions produce an actionable extension-load error and publish nothing. AoE independently checks each report's `provider_version` against `agent_integrations.provider_versions`.

OMP 18.10.3 uses the native `select(["Approve", "Deny"])` approval API. OMP 18.10.4 and 18.11.1 add `ui.approve({ full, compact })`, with a select fallback. The integration observes these native APIs and rich `askDialog`, not prompt text or composer geometry, so compact/full UI styles use the same contract. Only an approval paired with a provider approval event is replyable; unrelated selectors keep their native behavior. "Allow always" is not offered.

Version declarations are intentionally narrow: the published APIs of those releases were inspected; startup smoke coverage exercises the installed 18.10.3 and 18.10.4 binaries. Widen support only after inspecting that release's APIs and exercising the smoke path. With an approved integration configured, a missing, incompatible, expired, or exited provider blocks explicit replies; status still falls back to native terminal detection. Without a configured integration, AoE's independent terminal response path remains available.

## Verification

```sh
bun test plugins/agent-omp/tests
OMP_BIN=/path/to/supported/omp sh plugins/agent-omp/tests/omp-native-smoke.sh
```

The smoke harness isolates user state and loads the extension into a real OMP process without credentials or a model request. It verifies that `session_start` publishes the running host's version and session identity. Run it for each runtime being added; unit tests cover version boundaries and native dialog completion/cancellation.