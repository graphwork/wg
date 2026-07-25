# Pi is WorksGood's model plane

WorksGood (`wg`) has one supported LLM execution system: **Pi**.

## Ownership boundary

Pi owns:

- provider authentication and credentials;
- provider/model discovery and search;
- endpoint details;
- model availability and support validation;
- provider-reported token usage and cost.

WG owns:

- the durable task graph and lifecycle/security registries;
- strong/weak orchestration policy;
- exact per-role `pi:<provider>:<model>` routes;
- inherited reasoning (`off|minimal|low|medium|high|xhigh|max`);
- execution identity, provenance, and Pi-reported accounting.

The agent registry, service registry, task-attempt registry, and authenticated
service identity are lifecycle/security mechanisms. They are not model catalogs
and are unchanged by this boundary.

## Start graph-only, then select explicitly

```bash
wg init
wg tui
```

These commands are credential-free and do not write or infer a model route.
Before LLM execution, select Pi explicitly:

```bash
wg setup --route pi --yes --model pi:<provider>:<model>
# or
wg profile select pi
```

Use Pi itself to log in and discover/select available models. WG deliberately
does not duplicate Pi's provider or model validation.

Inspect the complete effective policy:

```bash
wg config --models
```

Every displayed LLM role has handler `pi`, an exact route, and visible effective
reasoning. A missing route, non-Pi route, or omitted effective reasoning fails
closed without another handler or cross-system fallback.

## Unregistered models are valid

A route does not need a WG catalog entry:

```toml
[models.task_agent]
model = "pi:future-provider:vendor/new-model"
reasoning = "high"
```

WG preserves this identity exactly and passes it to Pi. Pi decides whether the
provider/model is authenticated, available, and supported.

## Accounting

WG records token usage and cost emitted by Pi events. If Pi reports usage but no
cost, cost remains unknown/zero. WG does not substitute legacy registry pricing
or silently map the model to a catalog entry.

## Legacy configuration

Old `model_registry`, provider, endpoint, and credential fields remain readable
so `wg config lint` and `wg migrate config` can report/migrate them
deterministically. They have no authority in Pi dispatch. Compatibility catalog
commands, where retained, are expert/deprecated surfaces only.
