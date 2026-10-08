import { api, errorMessage, type OpAccount, type OpItem, type OpSshKey, type OpVault } from "$lib/api";

/** What the dialog lists from 1Password: SSH keys, logins (for their password), or nothing. */
export type OpListing = "keys" | "logins" | null;

/** A workspace's default SSH key, and the account it was picked from ("" for the default account). */
export interface WorkspaceKey {
  fingerprint: string;
  item: string | null;
  /** Saved with the key, for workspaces that have it. */
  title?: string | null;
  account: string;
}

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
  /** Likewise for the vault. */
  vaultPicked = false;
  /** Set once the user asked to pick another key than the workspace's default. */
  browsing = $state(false);
  /** Bumped by `retry`; the loading effects depend on it. */
  #attempt = $state(0);
  #wsKey: () => WorkspaceKey | null;
  #ownKey: () => boolean;

  /** The workspace's default SSH key, used by connections that choose none. */
  get defaultKey(): WorkspaceKey | null {
    const key = this.#wsKey();
    // Without an account the key was picked from 1Password's default account.
    return key && (!key.account || key.account === this.account) ? key : null;
  }

  /** The workspace's default key, whichever account it was picked from. */
  get workspaceKey(): WorkspaceKey | null {
    return this.#wsKey();
  }

  /**
   * Whether the default key stands in for a listing: the connection has no key of its own and
   * the user didn't ask to browse. Listing keys costs an `op` call per key, so it waits.
   */
  get usesDefaultKey(): boolean {
    const key = this.#wsKey();
    if (!key || this.browsing || this.#ownKey()) return false;
    return !this.account || !key.account || key.account === this.account;
  }

  /** Start listing keys after all. */
  browse() {
    this.browsing = true;
  }

  /** Run the failed listing again. */
  retry() {
    this.error = null;
    if (this.accounts?.length === 0) this.accounts = null;
    this.#attempt++;
  }

  constructor(
    account: string,
    listing: () => OpListing,
    wsAccount: () => string,
    wsVault: () => string,
    wsKey: () => WorkspaceKey | null,
    ownKey: () => boolean = () => false,
  ) {
    this.account = account;
    this.#wsKey = wsKey;
    this.#ownKey = ownKey;
    // Its own derived, so switching between the two 1Password options doesn't reload accounts or vaults.
    const active = $derived(listing() !== null);
    // Also derived, so leaving the keys option for logins doesn't re-run the loaders.
    const deferred = $derived(listing() === "keys" && this.usesDefaultKey);

    // Follow the workspace's 1Password account until the user picks one explicitly.
    $effect(() => {
      const acc = wsAccount();
      if (!this.picked && acc && this.accounts?.some((a) => a.id === acc)) this.account = acc;
    });

    // Likewise for the vault, so a workspace with a default vault doesn't list every vault's keys.
    $effect(() => {
      const vault = wsVault();
      if (!this.vaultPicked) this.vault = vault;
    });

    // Accounts once, when a 1Password option is first picked.
    $effect(() => {
      void this.#attempt;
      if (!active || this.accounts || deferred) return;
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
      void this.#attempt;
      if (!active || !account || deferred) return;
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
      void this.#attempt;
      if (!account || !what || (what === "keys" && deferred)) return;
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
