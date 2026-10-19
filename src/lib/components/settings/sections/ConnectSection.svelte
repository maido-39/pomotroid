<script lang="ts">
  // "Desktop" section (Android app and browser build): pair with a desktop
  // Pomotroid over LAN and follow its timer.
  import { onMount } from 'svelte';
  import { disposable } from '$lib/utils/listeners';
  import {
    remoteConnect,
    remoteDisconnect,
    remoteStatus,
    remoteParsePairing,
    onRemoteStatus,
  } from '$lib/backend';
  import type { RemoteStatus } from '$lib/types';
  import * as m from '$paraglide/messages.js';
  import { isMobile, isWeb } from '$lib/utils/platform';
  import { error as logError } from '$lib/utils/log';

  let status = $state<RemoteStatus>({
    mode: 'local',
    state: 'disconnected',
    host: '',
    port: 1314,
    token: '',
    desktop_name: '',
    error: null,
  });

  let host = $state('');
  let port = $state('1314');
  let token = $state('');
  let pasted = $state('');
  let busy = $state(false);
  let formError = $state<string | null>(null);

  const connected = $derived(status.mode === 'remote' && status.state === 'connected');
  const remoteMode = $derived(status.mode === 'remote');

  function stateLabel(s: RemoteStatus): string {
    if (s.mode === 'local') return m.connect_state_disconnected();
    switch (s.state) {
      case 'connecting':
        return m.connect_state_connecting();
      case 'connected':
        return m.connect_state_connected({ name: s.desktop_name || s.host });
      case 'reconnecting':
        return m.connect_state_reconnecting();
      case 'error':
        return m.connect_state_error();
      default:
        return m.connect_state_disconnected();
    }
  }

  function applyStatus(s: RemoteStatus) {
    status = s;
    if (s.host && !host) host = s.host;
    if (s.port && port === '1314') port = String(s.port);
    if (s.token && !token) token = s.token;
  }

  onMount(() => {
    const scope = disposable();
    scope.add(onRemoteStatus(applyStatus));
    remoteStatus()
      .then((s) => {
        if (!scope.disposed) applyStatus(s);
      })
      .catch((err) => logError(`[connect] remoteStatus failed: ${err}`));
    return scope.dispose;
  });

  async function fillFromText(text: string) {
    formError = null;
    try {
      const t = await remoteParsePairing(text);
      host = t.host;
      port = String(t.port);
      token = t.token;
      pasted = '';
      return true;
    } catch (err) {
      formError = String(err);
      return false;
    }
  }

  async function scanQr() {
    formError = null;
    try {
      const scanner = await import('@tauri-apps/plugin-barcode-scanner');
      const perm = await scanner.checkPermissions();
      if (perm !== 'granted') {
        const req = await scanner.requestPermissions();
        if (req !== 'granted') {
          formError = 'Camera permission denied';
          return;
        }
      }
      const result = await scanner.scan({ windowed: false, formats: [scanner.Format.QRCode] });
      if (result?.content && (await fillFromText(result.content))) {
        await doConnect();
      }
    } catch (err) {
      await logError(`[connect] scan failed: ${err}`);
      formError = String(err);
    }
  }

  async function doConnect() {
    formError = null;
    if (pasted.trim() && !(await fillFromText(pasted))) return;
    const p = parseInt(port, 10);
    if (!host.trim() || isNaN(p) || p < 1 || p > 65535) {
      formError = 'Enter a host and a valid port';
      return;
    }
    busy = true;
    try {
      await remoteConnect(host.trim(), p, token.trim());
    } catch (err) {
      formError = String(err);
    } finally {
      busy = false;
    }
  }

  async function doDisconnect() {
    busy = true;
    try {
      await remoteDisconnect();
    } catch (err) {
      formError = String(err);
    } finally {
      busy = false;
    }
  }
</script>

<div class="section">
  <div class="group-heading">{m.connect_group_status()}</div>

  <div class="status-row">
    <span class="dot" class:on={connected} class:busy={remoteMode && !connected}></span>
    <div class="status-text">
      <span class="label">{stateLabel(status)}</span>
      {#if status.error && status.state !== 'connected'}
        <span class="desc error">{status.error}</span>
      {:else if remoteMode}
        <span class="desc">{status.host}:{status.port}</span>
      {:else}
        <span class="desc">{m.connect_local_banner()}</span>
      {/if}
    </div>
    {#if remoteMode}
      <button class="btn-sm" disabled={busy} onclick={doDisconnect}>{m.connect_btn_disconnect()}</button>
    {/if}
  </div>

  <p class="note">{m.connect_note()}</p>

  {#if !isWeb}
    <div class="group-heading">{m.connect_group_pair()}</div>

    {#if isMobile}
      <div class="row">
        <button class="btn-primary" disabled={busy} onclick={scanQr}>{m.connect_btn_scan()}</button>
      </div>
    {/if}

    <div class="row">
      <input class="input wide" placeholder={m.connect_paste_placeholder()} bind:value={pasted} />
    </div>
    <div class="row grid">
      <label>
        <span class="field-label">{m.connect_label_host()}</span>
        <input class="input" bind:value={host} placeholder="192.168.0.10" autocapitalize="off" />
      </label>
      <label>
        <span class="field-label">{m.connect_label_port()}</span>
        <input class="input" bind:value={port} inputmode="numeric" />
      </label>
      <label class="span2">
        <span class="field-label">{m.connect_label_token()}</span>
        <input class="input" bind:value={token} autocapitalize="off" spellcheck={false} />
      </label>
    </div>
    <div class="row actions">
      <button class="btn-primary" disabled={busy} onclick={doConnect}>{m.connect_btn_connect()}</button>
    </div>
    {#if formError}
      <p class="form-error">{formError}</p>
    {/if}
  {/if}
</div>

<style>
  .section {
    display: flex;
    flex-direction: column;
    padding-bottom: 20px;
  }

  .group-heading {
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.6;
    margin: 0;
    padding: 16px 20px 6px;
  }

  .status-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 20px;
    border-bottom: 1px solid var(--color-separator);
  }

  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: color-mix(in oklch, var(--color-foreground) 30%, transparent);
    flex-shrink: 0;
  }

  .dot.on {
    background: var(--color-short-round);
    box-shadow: 0 0 8px color-mix(in oklch, var(--color-short-round) 60%, transparent);
  }

  .dot.busy {
    background: var(--color-long-round);
    animation: pulse 1.2s ease-in-out infinite;
  }

  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }

  .status-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .label {
    font-size: 0.85rem;
    color: var(--color-foreground);
    letter-spacing: 0.02em;
  }

  .desc {
    font-size: 0.72rem;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
    font-family: monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .desc.error {
    color: var(--color-focus-round);
    opacity: 1;
    font-family: inherit;
  }

  .note {
    font-size: 0.75rem;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.65;
    padding: 12px 20px;
    line-height: 1.6;
  }

  .row {
    padding: 8px 20px;
  }

  .row.grid {
    display: grid;
    grid-template-columns: 1fr 96px;
    gap: 10px;
  }

  .span2 {
    grid-column: 1 / -1;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field-label {
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
  }

  .input {
    width: 100%;
    background: var(--color-hover);
    border: 1px solid color-mix(in oklch, var(--color-foreground) 18%, transparent);
    border-radius: 4px;
    color: var(--color-foreground);
    font-size: 0.85rem;
    font-family: monospace;
    padding: 7px 10px;
    outline: none;
    transition: border-color 0.15s;
  }

  .input:focus {
    border-color: var(--color-accent);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }

  .btn-primary,
  .btn-sm {
    border-radius: 4px;
    cursor: pointer;
    border: 1px solid color-mix(in oklch, var(--color-accent) 50%, transparent);
    background: color-mix(in oklch, var(--color-accent) 16%, transparent);
    color: var(--color-accent);
    transition: background 0.15s;
    white-space: nowrap;
  }

  .btn-primary {
    font-size: 0.85rem;
    padding: 8px 18px;
    width: 100%;
  }

  .btn-sm {
    font-size: 0.72rem;
    padding: 4px 10px;
  }

  .btn-primary:hover,
  .btn-sm:hover {
    background: color-mix(in oklch, var(--color-accent) 26%, transparent);
  }

  .btn-primary:disabled,
  .btn-sm:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .form-error {
    margin: 0;
    padding: 2px 20px 8px;
    font-size: 0.75rem;
    color: var(--color-focus-round);
  }
</style>
