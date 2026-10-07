import { api, errorMessage, type OpAccount, type OpItem, type OpSshKey, type OpVault } from "$lib/api";

/** What the dialog lists from 1Password: SSH keys, logins (for their password), or nothing. */
export type OpListing = "keys" | "logins" | null;

/**
 * Which 1Password account and vault to use, and the SSH keys or logins in it.
 * A machine can be signed in to several accounts (work and personal); without
 * one, `op` picks its default. Create it while a component initialises: it
 * loads through effects, only once a 1Password option is picked.
 */
export class OnePassword {
  accounts = $state<OpAccount[] | null>(null);
  account = $state("");
  vaults = $state<OpVault[]>([]);
  vault = $state("");
  items = $state<OpItem[] | null>(null);
  keys = $state<OpSshKey[] | null>(null);
  loading = $state(false);
  error = $state<string | null>(null);
  /** Search in the logins; kept while switching between options. */
  query = $state("");
  /** Set once the user chose an account; from then on the workspace's no longer applies. */
  picked = false;

  constructor(account: string, listing: () => OpListing, wsAccount: () => string) {
    this.account = account;
    // Its own derived, so switching between the two 1Password options doesn't reload accounts or vaults.
    const active = $derived(listing() !== null);

    // Follow the workspace's 1Password account until the user picks one explicitly.
    $effect(() => {
      const acc = wsAccount();
      if (!this.picked && acc && this.accounts?.some((a) => a.id === acc)) this.account = acc;
    });

    // Accounts once, when a 1Password option is first picked.
    $effect(() => {
      if (!active || this.accounts) return;
      api
        .opAccounts()
        .then((list) => {
          this.accounts = list;
          if (!list.some((a) => a.id === this.account)) {
            const ws = wsAccount();
            this.account = list.some((a) => a.id === ws) ? ws : (list[0]?.id ?? "");
          }
        })
        .catch((e) => {
          this.accounts = [];
          this.error = errorMessage(e);
        });
    });

    // Vaults of the chosen account.
    $effect(() => {
      const account = this.account;
      if (!active || !account) return;
      this.vaults = [];
      let cancelled = false;
      api
        .opVaults(account)
        .then((v) => {
          if (cancelled) return;
          this.vaults = v;
          if (this.vault && !v.some((x) => x.id === this.vault)) this.vault = "";
        })
        .catch((e) => {
          if (!cancelled) this.error = errorMessage(e);
        });
      return () => (cancelled = true);
    });

    // Logins (for passwords) or SSH keys in the chosen account/vault.
    $effect(() => {
      const [account, vault, what] = [this.account, this.vault, listing()];
      if (!account || !what) return;
      this.loading = true;
      this.error = null;
      let cancelled = false;
      const fail = (e: unknown) => {
        if (!cancelled) this.error = errorMessage(e);
      };
      const done = () => {
        if (!cancelled) this.loading = false;
      };
      if (what === "logins") {
        this.items = null;
        api.opItems(account, vault || null).then((i) => !cancelled && (this.items = i)).catch(fail).finally(done);
      } else {
        this.keys = null;
        api.opSshKeys(account, vault || null).then((k) => !cancelled && (this.keys = k)).catch(fail).finally(done);
      }
      return () => (cancelled = true);
    });
  }
}
