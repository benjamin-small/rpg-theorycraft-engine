import { lessons } from './lessons';
import { streamSimulation } from './timeline';

const button = document.querySelector<HTMLButtonElement>('#run')!;
const status = document.querySelector<HTMLParagraphElement>('#status')!;
const timeline = document.querySelector<HTMLPreElement>('#timeline')!;

button.addEventListener('click', async () => {
  button.disabled = true;
  status.textContent = 'Running simulation…';
  timeline.replaceChildren();

  try {
    const { events, result } = streamSimulation(lessons[6].config, {
      kind: 'monte_carlo',
      iterations: 2000,
      seed: 7n,
      eventIteration: 0,
    });

    for await (const event of events) {
      timeline.append(JSON.stringify(event) + '\n');
    }

    const { report } = await result;
    status.textContent = `Complete · ${report.total.dps.toFixed(2)} mean DPS`;
  } catch (error) {
    status.textContent = error instanceof Error ? error.message : String(error);
  } finally {
    button.disabled = false;
  }
});
