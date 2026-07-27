#!/usr/bin/env bash
# Credential-free deterministic strong merger adapter used by merge-resolution tests.
set -euo pipefail
for secret in OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY; do
  [[ -z ${!secret:-} ]] || { echo "credential leaked: $secret" >&2; exit 90; }
done
workspace= bundle= route= provider= model= reasoning= tool= sandbox= outcome=
while (($#)); do
  case $1 in
    --workspace) workspace=$2;; --bundle-cid) bundle=$2;; --route) route=$2;;
    --provider) provider=$2;; --model) model=$2;; --reasoning) reasoning=$2;;
    --tool-policy-cid) tool=$2;; --sandbox-policy-cid) sandbox=$2;; --outcome) outcome=$2;;
  esac
  shift 2
done
[[ $route == *:* && $bundle == wgcid:v1:blake3:* && $tool == wgcid:v1:blake3:* && $sandbox == wgcid:v1:blake3:* ]]
[[ $reasoning == high || $reasoning == xhigh ]]
[[ -d $workspace/.git && ! -e $workspace/.wg/graph.jsonl ]]
[[ -z $(git -C "$workspace" remote) ]]
printf '%s\n' "${WG_FAKE_RESOLUTION_CONTENT:-resolved}" >"$workspace/value.txt"
printf '{"outcome":"resolved","explanation":"deterministic credential-free fake","generator_commands":[]}\n' >"$outcome"
