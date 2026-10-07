<script lang="ts">
  import { api, errorMessage, type AgentKey } from "$lib/api";
  import { t } from "$lib/i18n.svelte";
  import PickList, { type PickRow } from "./PickList.svelte";

  let {
    method,
    pinned = $bindable(),
    inset = false,
  }: {
    /** Whose agent: the system ssh-agent, or 1Password's when its CLI is missing. */
    method: "agent" | "one_password";
    pinned: string | null;
    inset?: boolean;
  } = $props();

  let keys = $state<AgentKey[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    loading = true;
    error = null;
    keys = [];
    let cancelled = false;
    api
      .agentKeys({ method, key_fingerprint: null })
      .then((k) => !cancelled && (keys = k))
      .catch((e) => !cancelled && (error = errorMessage(e)))
      .finally(() => !cancelled && (loading = false));
    return () => (cancelled = true);
  });

  const rows: PickRow[] = $derived([
    {
      id: "",
      title: t("Try every key"),
      detail: t("May hit the server's MaxAuthTries limit"),
      on: pinned === null,
      pick: () => {
        pinned = null;
      },
    },
    ...keys.map((k) => ({
      id: k.fingerprint,
      title: k.comment || t("Unnamed key"),
      detail: `${k.algorithm} · ${k.fingerprint.replace("SHA256:", "").slice(0, 12)}…`,
      mono: true,
      on: pinned === k.fingerprint,
      pick: () => {
        pinned = k.fingerprint;
      },
    })),
  ]);
</script>

<!-- The "try every key" row only makes sense once there are keys. -->
<PickList rows={keys.length ? rows : []} loading={loading ? t("Fetching keys…") : null} {error} empty={t("No keys found in the agent.")} {inset} />
