import { describe, expect, it } from 'vitest';
import {
  composeProjectAgentName,
  fullProjectAgentName,
  nameWithProjectSurname,
  projectAgentNameFields,
} from '../src/chrome/ProjectAgentName';

describe('project agent name fields', () => {
  it('does not auto-append a project suffix in generic name formatting', () => {
    expect(fullProjectAgentName('CC', 'valencia')).toBe('CC');
    expect(projectAgentNameFields('CC', 'valencia')).toMatchObject({
      given: 'CC',
      middle: '',
      surname: '',
    });
  });

  it('replaces the surname with v. project only at incarnation creation time', () => {
    expect(nameWithProjectSurname('CC', 'valencia')).toBe('CC v. valencia');
    expect(nameWithProjectSurname('CC', 'sample-co/HarborLab')).toBe('CC v. HarborLab');
    expect(nameWithProjectSurname('CC Smith', 'valencia')).toBe('CC v. valencia');
    expect(nameWithProjectSurname('CC-Bunshin Smith', 'valencia')).toBe('CC-Bunshin v. valencia');
    expect(nameWithProjectSurname('CC v. old-project', 'valencia')).toBe('CC v. valencia');
  });

  it('preserves an edited incarnation surname exactly as typed', () => {
    const fields = projectAgentNameFields('CC v. valencia');
    expect(fields.surname).toBe('v. valencia');
    expect(composeProjectAgentName({ ...fields, surname: 'Smith' })).toBe('CC Smith');
    expect(projectAgentNameFields('CC Smith')).toMatchObject({
      given: 'CC',
      middle: '',
      surname: 'Smith',
    });
    expect(projectAgentNameFields('CC-Bunshin Smith')).toMatchObject({
      given: 'CC',
      middle: 'Bunshin',
      surname: 'Smith',
    });
    expect(projectAgentNameFields('CC II')).toMatchObject({
      given: 'CC II',
      middle: '',
      surname: '',
    });
    expect(projectAgentNameFields('CC II Smith')).toMatchObject({
      given: 'CC II',
      middle: '',
      surname: 'Smith',
    });
  });
});
