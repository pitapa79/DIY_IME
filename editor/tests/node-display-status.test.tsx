// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { NodeProps } from '@xyflow/react';
import GraphNodeView, { type GraphFlowNode } from '../src/GraphNode';
afterEach(cleanup);
it('shows discarded as excluded from adoption without claiming it never ran or physically stopped', () => {
  const props: NodeProps<GraphFlowNode> = { id: 'held', type: 'graph', data: { node: { id: 'held', kind: 'test.http-held', version: 1 }, dirty: false, status: 'discarded' },
    selected: false, dragging: false, draggable: true, selectable: true, deletable: true, isConnectable: true, zIndex: 0, positionAbsoluteX: 0, positionAbsoluteY: 0 };
  const ui=render(<GraphNodeView {...props} />);expect(screen.getByText('採用対象外')).toBeTruthy();expect(screen.queryByText('未実行')).toBeNull();
  ui.rerender(<GraphNodeView {...props} data={{ ...props.data, status: undefined }} />);expect(screen.getByText('未実行')).toBeTruthy();
  ui.rerender(<GraphNodeView {...props} data={{ ...props.data, status: 'executed' }} />);expect(screen.getByText('完了')).toBeTruthy();
});
