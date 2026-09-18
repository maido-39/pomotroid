<script lang="ts">
  import { onMount } from 'svelte';
  import { isWeb } from '$lib/utils/platform';
  import { startWebSession } from '$lib/backend';
  import * as web from '$lib/remote/web';
  import * as m from '$paraglide/messages.js';
  import type { Snippet } from 'svelte';

  let { children }: { children?: Snippet } = $props();

  // Browser build: audio needs a user gesture before it may play.
  let needsAudioUnlock = $state(isWeb);

  onMount(() => {
    const disableContextMenu = (event: MouseEvent) => {
      event.preventDefault();
    };
    document.addEventListener('contextmenu', disableContextMenu, { capture: true });

    let unlockHandler: (() => void) | undefined;
    if (isWeb) {
      startWebSession();
      unlockHandler = () => {
        web.unlockAudio();
        needsAudioUnlock = false;
        document.removeEventListener('pointerdown', unlockHandler!, true);
        document.removeEventListener('keydown', unlockHandler!, true);
      };
      document.addEventListener('pointerdown', unlockHandler, true);
      document.addEventListener('keydown', unlockHandler, true);
    }

    return () => {
      document.removeEventListener('contextmenu', disableContextMenu, { capture: true });
      if (unlockHandler) {
        document.removeEventListener('pointerdown', unlockHandler, true);
        document.removeEventListener('keydown', unlockHandler, true);
      }
    };
  });
</script>

{@render children?.()}

{#if needsAudioUnlock}
  <div class="audio-unlock">{m.web_tap_to_enable_sound()}</div>
{/if}

<style>
  .audio-unlock {
    position: fixed;
    left: 50%;
    bottom: calc(12px + env(safe-area-inset-bottom, 0px));
    transform: translateX(-50%);
    z-index: 10000;
    padding: 6px 12px;
    border-radius: 999px;
    font-size: 0.72rem;
    letter-spacing: 0.03em;
    background: color-mix(in oklch, var(--color-foreground) 14%, var(--color-background));
    color: var(--color-foreground);
    border: 1px solid var(--color-separator);
    pointer-events: none;
    animation: app-fade-in 0.3s ease both;
  }
</style>
