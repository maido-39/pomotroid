<script lang="ts">
  // A labelled range slider row with a value badge and an accent fill bar.
  // The parent owns the value; `oninput` fires on every change.

  interface Props {
    label: string;
    value: number;
    min: number;
    max: number;
    step?: number;
    /** Text shown in the value badge (e.g. "60%"). */
    display: string;
    oninput: (value: number) => void;
  }

  let { label, value, min, max, step = 1, display, oninput }: Props = $props();

  const fill = $derived(max > min ? ((value - min) / (max - min)) * 100 : 0);
</script>

<div class="slider-row">
  <div class="slider-meta">
    <span class="label">{label}</span>
    <span class="value">{display}</span>
  </div>
  <div class="slider-wrap">
    <input
      type="range"
      {min}
      {max}
      {step}
      {value}
      class="slider"
      aria-label={label}
      oninput={(e) => oninput((e.target as HTMLInputElement).valueAsNumber)}
    />
    <div class="bar" style="width: {fill}%"></div>
  </div>
</div>

<style>
  .slider-row {
    padding: 10px 20px 14px;
    border-bottom: 1px solid var(--color-separator);
  }

  .slider-meta {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 10px;
  }

  .label {
    font-size: 0.85rem;
    color: var(--color-foreground);
    letter-spacing: 0.02em;
  }

  .value {
    font-size: 0.8rem;
    font-family: monospace;
    color: var(--color-foreground-darker, var(--color-foreground));
    background: var(--color-hover);
    padding: 2px 8px;
    border-radius: 3px;
  }

  .slider-wrap {
    position: relative;
    height: 20px;
    display: flex;
    align-items: center;
  }

  .slider {
    position: relative;
    z-index: 2;
    width: 100%;
    -webkit-appearance: none;
    appearance: none;
    height: 4px;
    background: color-mix(in oklch, var(--color-foreground) 14%, transparent);
    border-radius: 2px;
    outline: none;
    cursor: pointer;
  }

  .slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--color-foreground);
    cursor: pointer;
  }

  .slider::-moz-range-thumb {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--color-foreground);
    cursor: pointer;
    border: none;
  }

  .bar {
    position: absolute;
    left: 0;
    height: 4px;
    border-radius: 2px;
    pointer-events: none;
    z-index: 1;
    background: var(--color-accent);
    transition: width 0.05s;
  }
</style>
