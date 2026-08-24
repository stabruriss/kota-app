import { describe, expect, it } from 'vitest';

import {
  createEmberSchedule,
  emberReminderTerminalTiming,
} from '../src/ember-config';

const NOW = Date.parse('2026-08-18T16:00:00Z');

function reminder(mode: 'delay' | 'idle') {
  return createEmberSchedule({
    text: 'Take a break.',
    targetAgentId: 'agent-one',
    targetAgentName: 'Agent One',
    mode,
    delayAmount: 30,
    delayUnit: 'minutes',
  }, NOW);
}

describe('Ember reminder terminal timing', () => {
  it('identifies the next clock-based occurrence', () => {
    const schedule = reminder('delay');

    expect(emberReminderTerminalTiming(schedule, 'auto')).toEqual({
      trigger: 'scheduled',
      scheduledFor: schedule.nextRunAt,
    });
  });

  it('does not claim a scheduled timestamp for idle delivery', () => {
    expect(emberReminderTerminalTiming(reminder('idle'), 'auto')).toEqual({
      trigger: 'idle',
    });
  });

  it('treats Run as manual even when the schedule has a future occurrence', () => {
    expect(emberReminderTerminalTiming(reminder('delay'), 'manual')).toEqual({
      trigger: 'manual',
    });
  });
});
