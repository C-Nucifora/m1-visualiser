// SPDX-License-Identifier: GPL-3.0-or-later
// Run with node tests/viewer.cjs. Exercises the shipped script and Cytoscape.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const repo = path.resolve(__dirname, '..');
const cytoscape = require(path.join(repo, 'templates/cytoscape.min.js'));

function boot(graph) {
  const elements = {};
  let ready, cy;
  const document = {
    body: { classList: { add() {} } },
    getElementById(id) {
      return elements[id] ||= {
        value: '', checked: true, textContent: '', classList: { add() {} },
        listeners: {},
        addEventListener(event, callback) { this.listeners[event] = callback; },
        get valueAsNumber() { return Number(this.value); }
      };
    },
    addEventListener(event, callback) { ready = callback; }
  };
  const script = fs.readFileSync(path.join(repo, 'templates/viewer.html'), 'utf8')
    .split('<script>').at(-1).split('</script>')[0]
    .replace('/*__GRAPH_JSON__*/{"nodes":[],"edges":[]}', JSON.stringify(graph));
  vm.runInNewContext(script, {
    document,
    cytoscape(options) {
      cy = cytoscape({
        elements: JSON.parse(JSON.stringify(options.elements)),
        headless: true, layout: { name: 'preset' }
      });
      return cy;
    }
  });
  ready();
  return { cy, elements };
}

const graph = {
  nodes: [
    { id: 'Root', kind: 'group' },
    { id: 'Root.A', kind: 'channel', parent: 'Root', unit: 'rpm' },
    { id: 'Root.B', kind: 'channel', parent: 'Root' }
  ],
  edges: [{ from: 'Root.A', to: 'Root.B', kind: 'data_flow' }]
};
for (const kind of ['value', 'diff']) {
  const overlay = {
    kind, time: [0, 1, 2], start_tick: 0,
    nodes: { 'Root.A': { series: [{ num: 10 }, { num: 20 }], delta: [1, 2], max_abs_delta: 2 } },
    changed: ['Root.A']
  };
  const { cy, elements: e } = boot({ ...graph, overlay });
  try {
    cy.getElementById('Root.A').emit('tap');
    const expected = kind === 'value'
      ? ['Root.A = 10 (rpm)', 'Root.A = 20 (rpm)', 'Root.A =  (rpm)']
      : ['Root.A: 9 → 10 (Δ 1)', 'Root.A: 18 → 20 (Δ 2)', 'Root.A: — → — (Δ —)'];
    for (const tick of [0, 1, 2, 0]) {
      e.scrubber.value = String(tick);
      e.scrubber.listeners.input();
      assert.equal(e['time-readout'].textContent, `t = ${tick}`);
      assert.equal(e.readout.textContent, expected[tick], kind);
    }
    cy.getElementById('Root.B').emit('tap');
    e.scrubber.value = '1';
    e.scrubber.listeners.input();
    assert.equal(e.readout.textContent, 'Root.B');
    cy.getElementById('Root').emit('dblclick');
    cy.getElementById('Root').emit('dblclick');
    assert.equal(cy.elements('.hidden-collapse').length, 0);
  } finally {
    cy.destroy();
  }
}
console.log('viewer value/diff scrubbing, missing values, selection, collapse: passed');

for (const metadata of [{ unit: 'rpm' }, { rate_hz: 100 }]) {
  const id = 'Root.Value';
  const series = [{ bool: false }, { bool: true }, { str: '<tag>& ~~~ " α' }, { num: 2 }, { str: '' }];
  const values = ['false', 'true', '<tag>& ~~~ " α', '2', ''];
  const { cy, elements: e } = boot({
    nodes: [{ id, path: id, kind: 'channel', ...metadata }], edges: [],
    overlay: { kind: 'value', time: [0, 1, 2, 3, 4], start_tick: 0,
      external: [id], nodes: { [id]: { series } } }
  });
  try {
    const node = cy.getElementById(id);
    const base = `Value\n${metadata.unit ? '[rpm]' : '100 Hz'}\next`;
    node.emit('tap');
    for (const tick of [0, 1, 2, 3, 4, 2, 0, 3]) {
      e.scrubber.value = String(tick);
      e.scrubber.listeners.input();
      assert.equal(node.data('label'), tick === 3 ? base : `${base}\n${values[tick]}`);
      assert.equal(node.hasClass('overlay-num'), tick === 3);
      assert.equal(node.hasClass('overlay-label'), tick !== 3);
      assert.equal(node.hasClass('external'), true);
      assert.equal(e.readout.textContent, `${id} = ${values[tick]}${metadata.unit ? ' (rpm)' : ''}`);
    }
  } finally {
    cy.destroy();
  }
}
console.log('viewer literal labels preserve metadata across scrubs: passed');

const searchFixture = boot({
  nodes: [{ id: 'Root.Value', kind: 'channel' }], edges: [],
  overlay: { kind: 'value', time: [0, 1, 2], start_tick: 0,
    nodes: { 'Root.Value': { series: [{ bool: false }, { bool: true }, { num: 2 }] } } }
});
try {
  const { cy, elements: e } = searchFixture;
  const node = cy.getElementById('Root.Value');
  e.search.value = 'true';
  e.search.listeners.input();
  assert.equal(node.hasClass('dimmed'), true);
  for (const tick of [1, 0, 1, 2, 1]) {
    e.scrubber.value = String(tick);
    e.scrubber.listeners.input();
    assert.equal(node.hasClass('dimmed'), tick !== 1, `active search at tick ${tick}`);
  }
} finally {
  searchFixture.cy.destroy();
}
console.log('viewer search follows changing value labels: passed');


for (const id of ['constructor', '__proto__', 'toString']) {
  for (const cyclic of [false, true]) {
    const { cy } = boot({
      nodes: ['A', id, 'B', 'Outside'].map(id => ({ id, path: id, kind: 'channel' })),
      edges: [
        { from: 'A', to: id, kind: 'data_flow' },
        { from: id, to: 'B', kind: 'data_flow' },
        ...(cyclic ? [{ from: 'B', to: 'A', kind: 'data_flow' }] : [])
      ]
    });
    try {
      for (const [start, end, direction] of [['A', 'B', 'down'], ['B', 'A', 'up']]) {
        cy.getElementById(start).emit('tap');
        for (const target of [id, end]) {
          assert.equal(cy.getElementById(target).hasClass(`cone-${direction}`), true);
          assert.equal(cy.getElementById(target).hasClass('cone-dimmed'), false);
        }
        assert.equal(cy.getElementById('Outside').hasClass('cone-dimmed'), true);
        cy.emit('tap');
        assert.equal(cy.elements('.cone-focus,.cone-up,.cone-down,.cone-dimmed').length, 0);
      }
    } finally {
      cy.destroy();
    }
  }
}
console.log('viewer cones traverse prototype names in both directions and cycles: passed');
