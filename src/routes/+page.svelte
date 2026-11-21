<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  // Placeholder screen. It asks the Rust core for its version to prove that the
  // web view, the app shell and the core are wired together on this platform.
  let coreVersion = $state<string | null>(null);
  let error = $state<string | null>(null);

  onMount(async () => {
    try {
      coreVersion = await invoke<string>("core_version");
    } catch (e) {
      error = String(e);
    }
  });
</script>

<main>
  <h1>peoples.chat</h1>
  <p class="tagline">Messaging that keeps working when the internet is gone.</p>

  {#if coreVersion}
    <p class="status ok">Core {coreVersion} linked</p>
  {:else if error}
    <p class="status failed">Core not reachable: {error}</p>
  {:else}
    <p class="status">Starting…</p>
  {/if}
</main>

<style>
  :global(:root) {
    color-scheme: light dark;
    font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
    font-size: 16px;
    line-height: 1.5;
    color: #14181f;
    background-color: #f6f7f9;
  }

  :global(body) {
    margin: 0;
  }

  main {
    box-sizing: border-box;
    min-height: 100vh;
    padding: 24px;
    padding-top: calc(24px + env(safe-area-inset-top));
    padding-bottom: calc(24px + env(safe-area-inset-bottom));
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
  }

  h1 {
    margin: 0;
    font-size: 2rem;
    font-weight: 650;
    letter-spacing: -0.02em;
  }

  .tagline {
    margin: 8px 0 32px;
    max-width: 22em;
    color: #5a6472;
  }

  .status {
    margin: 0;
    padding: 6px 14px;
    border-radius: 999px;
    font-size: 0.875rem;
    color: #5a6472;
    background-color: #e7eaee;
  }

  .status.ok {
    color: #0b5a2b;
    background-color: #d8f1e1;
  }

  .status.failed {
    color: #8a1c1c;
    background-color: #f8dcdc;
  }

  @media (prefers-color-scheme: dark) {
    :global(:root) {
      color: #eef1f5;
      background-color: #12151a;
    }

    .tagline,
    .status {
      color: #a3adba;
    }

    .status {
      background-color: #222831;
    }

    .status.ok {
      color: #8fe0ad;
      background-color: #143524;
    }

    .status.failed {
      color: #f3a3a3;
      background-color: #3d1a1a;
    }
  }
</style>
