import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LocalMcpPanel } from '../src/chrome/LocalMcpPanel';
import * as mcp from '../src/lib/local-mcp';
const scope = { projectRoot:'/fixture',agentId:'agent-example',sessionId:'session-one',epoch:1 };
const props = { projectRoot:scope.projectRoot,agentId:scope.agentId,scope,now:0 };
const initial = (): mcp.LocalMcpView => ({ schema:1,available:true,servers:[
  { name:'filesystem',status:'connected',configured:true,enabled:true },
  { name:'github',status:'needs_auth',configured:true,enabled:true },
  { name:'bad',status:'failed',reason:'connection_timeout',configured:true,enabled:true },
] });
const open = () => fireEvent.click(screen.getByRole('button',{ name:'MCP servers' }));
beforeEach(() => {
  vi.restoreAllMocks(); Object.defineProperty(document,'hidden',{ configurable:true,value:false });
  vi.spyOn(mcp,'readLocalMcp').mockResolvedValue(initial());
  vi.spyOn(mcp,'removeLocalMcp').mockResolvedValue(true);
});
afterEach(() => vi.restoreAllMocks());

it('opens the approved tab with real health, prominent instruction and no hidden queries', async () => {
  const ui = render(<LocalMcpPanel {...props} />);
  expect(mcp.readLocalMcp).not.toHaveBeenCalled();
  open(); const dialog = await screen.findByRole('dialog',{ name:'MCP servers' });
  await within(dialog).findByText('filesystem');
  expect(within(dialog).getByText('tell your agent to connect mcp for you')).toBeInTheDocument();
  expect(within(dialog).getByText('Needs sign-in')).toBeInTheDocument();
  expect(within(dialog).getByText('Connection timed out.')).toBeInTheDocument();
  expect(mcp.readLocalMcp).toHaveBeenCalledWith('/fixture','agent-example',scope);
  ui.rerender(<LocalMcpPanel {...props} now={2000} />);
  await waitFor(() => expect(mcp.readLocalMcp).toHaveBeenCalledTimes(2));
  fireEvent.keyDown(document,{ key:'Escape' });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('button',{name:'MCP servers'})).toHaveFocus();
  ui.rerender(<LocalMcpPanel {...props} now={8000} />);
  expect(mcp.readLocalMcp).toHaveBeenCalledTimes(2);
});

it('unknown and failed reads show Retry, not an empty list or an error dump; retry recovers', async () => {
  vi.mocked(mcp.readLocalMcp).mockResolvedValueOnce({ schema:1,available:false,servers:[] });
  render(<LocalMcpPanel {...props} />); open();
  const dialog = screen.getByRole('dialog');
  await within(dialog).findByText('Status unavailable');
  expect(within(dialog).queryByText('No MCP servers.')).not.toBeInTheDocument();
  fireEvent.click(within(dialog).getByRole('button',{ name:'Retry' }));
  await within(dialog).findByText('filesystem');
  vi.mocked(mcp.readLocalMcp).mockRejectedValueOnce(new Error('fake secret URL'));
  fireEvent.click(screen.getByRole('button',{ name:'Close MCP' })); open();
  await within(screen.getByRole('dialog')).findByText('Status unavailable');
  expect(document.body).not.toHaveTextContent('fake secret');
});

it('Remove changes saved membership after ACK and retains an existing live connection', async () => {
  let accept!: (value:boolean) => void;
  vi.mocked(mcp.removeLocalMcp).mockImplementationOnce(() => new Promise(resolve => { accept=resolve; }));
  vi.mocked(mcp.readLocalMcp).mockImplementation(async () => initial());
  render(<LocalMcpPanel {...props} />); open();
  const dialog = screen.getByRole('dialog'); await within(dialog).findByText('filesystem');
  fireEvent.click(within(dialog).getByRole('button',{ name:'Remove filesystem' }));
  expect(within(dialog).queryByText('Removed')).not.toBeInTheDocument();
  vi.mocked(mcp.readLocalMcp).mockResolvedValue({ ...initial(),servers:initial().servers.map(row => row.name==='filesystem' ? { ...row,configured:false } : row) });
  await act(async () => accept(true));
  const row=within(dialog).getByText('filesystem').closest('li')!;
  await within(row).findByText('Removed'); expect(within(row).getByText('Connected')).toBeInTheDocument();
  expect(mcp.removeLocalMcp).toHaveBeenCalledWith('/fixture','agent-example','filesystem');
});

it('Remove failure preserves the row and reports a controlled reason', async () => {
  vi.mocked(mcp.removeLocalMcp).mockRejectedValue(new Error('sensitive path'));
  render(<LocalMcpPanel {...props} />);open();const dialog=screen.getByRole('dialog');
  await within(dialog).findByText('bad');fireEvent.click(within(dialog).getByRole('button',{name:'Remove bad'}));
  await within(dialog).findByText("Couldn't remove this server. Try again.");
  expect(within(dialog).getByRole('button',{name:'Remove bad'})).toBeEnabled();
  expect(document.body).not.toHaveTextContent('sensitive path');
});

it('serializes slow status reads and ignores results from a replaced session', async () => {
  let finish!: (value:mcp.LocalMcpView) => void;
  vi.mocked(mcp.readLocalMcp).mockImplementationOnce(() => new Promise(resolve => { finish=resolve; }));
  const ui=render(<LocalMcpPanel {...props} />);open();
  ui.rerender(<LocalMcpPanel {...props} now={6000} />);
  expect(mcp.readLocalMcp).toHaveBeenCalledTimes(1);
  const next={...scope,sessionId:'session-two',epoch:2};
  vi.mocked(mcp.readLocalMcp).mockResolvedValue({schema:1,available:true,servers:[]});
  ui.rerender(<LocalMcpPanel {...props} scope={next} now={8000} />);
  await within(screen.getByRole('dialog')).findByText('No MCP servers.');
  await act(async () => finish(initial()));
  expect(within(screen.getByRole('dialog')).queryByText('filesystem')).not.toBeInTheDocument();
});

it('does not poll while the app is hidden or after the panel unmounts; outside click closes', async () => {
  const ui=render(<LocalMcpPanel {...props} />);open();await screen.findByText('filesystem');
  Object.defineProperty(document,'hidden',{ configurable:true,value:true });
  ui.rerender(<LocalMcpPanel {...props} now={4000} />);
  expect(mcp.readLocalMcp).toHaveBeenCalledTimes(1);
  fireEvent.pointerDown(document.body);expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  ui.unmount();expect(mcp.readLocalMcp).toHaveBeenCalledTimes(1);
});
