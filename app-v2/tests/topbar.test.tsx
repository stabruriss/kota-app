import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { TopBar } from '../src/chrome/TopBar';

describe('TopBar project metadata', () => {
  it('exposes real GitHub workspace paths and repo URL on the active tab', () => {
    render(
      <TopBar
        projects={[
          {
            id: 'owner-repo',
            name: 'owner/repo',
            path: '/Users/me/Kota/Projects/owner/repo',
            sourcePath: '/Users/me/Kota/Projects/owner/repo',
            accountPath: '/Users/me/Kota/Workspaces/owner-repo',
            githubUrl: 'https://github.com/owner/repo',
          },
        ]}
        activeProjectId="owner-repo"
        onSelectProject={vi.fn()}
        onNewProject={vi.fn()}
        onCloseProject={vi.fn()}
        onOpenTavern={vi.fn()}
        ghAuth={undefined}
      />,
    );

    const tab = screen.getByTestId('tab-owner-repo');
    expect(tab.querySelector('.tab-name')).toHaveTextContent('repo');
    const title = tab.getAttribute('title') ?? '';
    expect(title).toContain('GitHub: https://github.com/owner/repo');
    expect(title).toContain('Local: /Users/me/Kota/Projects/owner/repo');
    expect(title).toContain('Account: /Users/me/Kota/Workspaces/owner-repo');
  });

  it('renders gh status as optional terminal tooling', async () => {
    const onGhAuthClick = vi.fn();
    render(
      <TopBar
        projects={[]}
        activeProjectId={null}
        onSelectProject={vi.fn()}
        onNewProject={vi.fn()}
        onCloseProject={vi.fn()}
        onOpenTavern={vi.fn()}
        ghAuth={{
          authenticated: false,
          username: null,
          scopes: [],
          error: 'not logged in',
          cliMissing: false,
        }}
        onGhAuthClick={onGhAuthClick}
      />,
    );

    const pill = screen.getByTestId('gh-auth-pill');
    expect(pill).toHaveAttribute('data-state', 'setup');
    await userEvent.click(pill);
    expect(onGhAuthClick).toHaveBeenCalledTimes(1);
  });

  it('turns the Tavern control into a project back button while Tavern is open', async () => {
    const onOpenTavern = vi.fn();
    render(
      <TopBar
        projects={[]}
        activeProjectId={null}
        onSelectProject={vi.fn()}
        onNewProject={vi.fn()}
        onCloseProject={vi.fn()}
        onOpenTavern={onOpenTavern}
        tavernOpen
        ghAuth={undefined}
      />,
    );

    const button = screen.getByTestId('tavern-btn');
    expect(button).toHaveAttribute('aria-label', 'Back to project');
    await userEvent.click(button);
    expect(onOpenTavern).toHaveBeenCalledTimes(1);
  });
});
