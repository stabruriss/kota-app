import { render } from '@testing-library/react';
import { expect, it } from 'vitest';
import { LocalWorkbenchText } from '../src/chrome/LocalWorkbenchText';

it('opens an existing snapshot fully visible and fades only the new fragment', () => {
  const { container, rerender } = render(<pre><LocalWorkbenchText text="紫鹃在" active /></pre>);
  const pre = container.querySelector('pre')!;
  expect(pre.textContent).toBe('紫鹃在');
  expect(pre.querySelector('span')).toBeNull();
  rerender(<pre><LocalWorkbenchText text="紫鹃在这里，一段一起出现。" active /></pre>);
  expect(pre.textContent).toBe('紫鹃在这里，一段一起出现。');
  expect(pre.querySelector('.stream-append')!.textContent).toBe('这里，一段一起出现。');
  expect(pre.childNodes).toHaveLength(2);
  const tail = pre.querySelector('span');
  rerender(<pre><LocalWorkbenchText text="紫鹃在这里，一段一起出现。" active /></pre>);
  expect(pre.querySelector('span')).toBe(tail);
});

it.each([
  ['前文e', '前文e\u0301', 'e\u0301'],
  ['前文👩', '前文👩‍💻', '👩‍💻'],
  ['前文👍', '前文👍🏽', '👍🏽'],
  ['前文\r', '前文\r\n后文', '\r\n后文'],
])('does not split a growing grapheme: %s', (before, after, tail) => {
  const { container, rerender } = render(<pre><LocalWorkbenchText text={before} active /></pre>);
  rerender(<pre><LocalWorkbenchText text={after} active /></pre>);
  expect(container.querySelector('pre')!.textContent).toBe(after);
  expect(container.querySelector('.stream-append')!.textContent).toBe(tail);
});

it('keeps DOM bounded through a fast stream instead of accumulating per-character spans', () => {
  const { container, rerender } = render(<pre><LocalWorkbenchText text="" active /></pre>);
  let text = '';
  for (let i = 0; i < 100; i++) {
    text += `第${i}段👩‍💻\n`;
    rerender(<pre><LocalWorkbenchText text={text} active /></pre>);
    const pre = container.querySelector('pre')!;
    expect(pre.textContent).toBe(text);
    expect(pre.querySelectorAll('span')).toHaveLength(1);
    expect(pre.childNodes.length).toBeLessThanOrEqual(2);
  }
});

it('shows corrections, truncation and terminal text immediately without a typing backlog', () => {
  const { container, rerender } = render(<pre><LocalWorkbenchText text="旧片段" active /></pre>);
  rerender(<pre><LocalWorkbenchText text="旧片段加长" active /></pre>);
  expect(container.querySelector('.stream-append')).not.toBeNull();
  for (const text of ['替换后的完整正文', '短', '']) {
    rerender(<pre><LocalWorkbenchText text={text} active /></pre>);
    expect(container.querySelector('pre')!.textContent).toBe(text);
    expect(container.querySelector('.stream-append')).toBeNull();
  }
  rerender(<pre><LocalWorkbenchText text="不等待动画的终态" active={false} /></pre>);
  expect(container.querySelector('pre')!.textContent).toBe('不等待动画的终态');
  expect(container.querySelector('.stream-append')).toBeNull();
});

it('does not re-animate old text when a block becomes active again', () => {
  const { container, rerender } = render(<pre><LocalWorkbenchText text="片段" active /></pre>);
  rerender(<pre><LocalWorkbenchText text="片段追加" active /></pre>);
  rerender(<pre><LocalWorkbenchText text="片段追加" active={false} /></pre>);
  rerender(<pre><LocalWorkbenchText text="片段追加" active /></pre>);
  expect(container.querySelector('pre')!.textContent).toBe('片段追加');
  expect(container.querySelector('.stream-append')).toBeNull();
});

it('preserves literal HTML, whitespace and paths as text, never media or markup', () => {
  const before = '  原文\n';
  const text = `${before}<img src="https://example.invalid/image.png">\n/tmp/one file.txt\n`;
  const { container, rerender } = render(<pre><LocalWorkbenchText text={before} active /></pre>);
  rerender(<pre><LocalWorkbenchText text={text} active /></pre>);
  expect(container.querySelector('pre')!.textContent).toBe(text);
  expect(container.querySelector('img, a, script')).toBeNull();
});
