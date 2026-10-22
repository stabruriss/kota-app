import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { TopBar } from '../src/chrome/TopBar';
import type { WorkspaceProject } from '../src/pty-client';
vi.mock('../src/pty-client', async original => ({...await original<typeof import('../src/pty-client')>(),
 githubAccountsRead:vi.fn().mockResolvedValue({defaultLogin:null,accounts:[]}),
 projectConnectionStatus:vi.fn().mockResolvedValue({kind:'github',accountLogin:null,accountState:'unbound'}),
}));

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

const localProject: WorkspaceProject = { projectId:'local_fixture', displayName:'Copper / Notes', connection:{kind:'local'},
  repoFullName:'', remoteUrl:'', githubHtmlUrl:'', sourceDir:'/fixture/local', localRoot:'/fixture/state',
  sourceDirBytes:0, localRootBytes:0, sharedDir:'/fixture/shared', rulesDir:'/fixture/rules', agents:[], defaultBranch:'main', baseRef:'main' };
const topProps={projects:[{id:'local_fixture',name:'Copper / Notes',displayName:'Copper / Notes'}],activeProjectId:'local_fixture',onSelectProject:vi.fn(),onNewProject:vi.fn(),onCloseProject:vi.fn(),onOpenTavern:vi.fn()};
it('shows Local for a local project and preserves its explicit display name', () => {
  render(<TopBar {...topProps} workspace={localProject} ghAuth={{authenticated:true,username:'unrelated',scopes:[],cliMissing:false,error:null}}/>);
  expect(screen.getByTestId('gh-auth-pill')).toHaveTextContent(/^Local$/);
  expect(screen.getByTestId('tab-local_fixture').querySelector('.tab-name')).toHaveTextContent(/^Copper \/ Notes$/);
});
it('labels the project account, never the unrelated global login; a legacy registration is not Local', () => {
  const {rerender}=render(<TopBar {...topProps} workspace={{...localProject,connection:{kind:'github',accountLogin:'project-login'}}} ghAuth={{authenticated:true,username:'unrelated',scopes:[],cliMissing:false,error:null}}/>);
  expect(screen.getByTestId('gh-auth-pill')).toHaveTextContent('GitHub · project-login');
  expect(screen.getByTestId('gh-auth-pill')).toHaveAttribute('title', 'Project account: project-login');
  rerender(<TopBar {...topProps} workspace={{...localProject,connection:undefined}}/>);
  expect(screen.getByTestId('gh-auth-pill')).toHaveTextContent('GitHub · No account');
  expect(screen.getByTestId('gh-auth-pill')).not.toHaveTextContent('unrelated');
});

it('opens Link from an unbound project label', async()=>{
 const open=vi.fn();window.addEventListener('kota:github-link',open);
 try {render(<TopBar {...topProps} workspace={{...localProject,connection:{kind:'github'}}}/>);await userEvent.click(screen.getByTestId('gh-auth-pill'));expect(open).toHaveBeenCalledTimes(1);}finally{window.removeEventListener('kota:github-link',open);}
});
