// Test the JavaScript actually injected by the native WebKit probe without
// opening a browser or compiling Swift. Cancelled callbacks remain available
// to model a callback that was already dispatched when sampling stopped.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const swift = readFileSync(new URL('./test-preview-partial-native.swift', import.meta.url), 'utf8');
const source = swift.match(/static let observation = """\n([\s\S]*?)\n    """/)[1];
function fixture() {
  let time = 0, next = 1;
  const pending = new Map(), cancelled = new Map();
  const window = { addEventListener() {} };
  vm.runInNewContext(source, {
    window, performance: { now: () => time },
    requestAnimationFrame(callback) { const id = next++;pending.set(id, callback);return id; },
    cancelAnimationFrame(id) {
      const callback = pending.get(id);
      if(callback)cancelled.set(id, callback);
      pending.delete(id);
    },
  });
  window.tttNativeObserve = () => ({ page: 18 });
  return { window, pending, cancelled,
    tick(delta = 16) {
      time += delta;
      const callbacks = [...pending.values()];pending.clear();
      callbacks.forEach(callback => callback(time));
    },
    stale(id) { assert.ok(cancelled.has(id));cancelled.get(id)(time + 16); },
  };
}

test('a stopped callback cannot revive during counter-only idle observation', () => {
  const f = fixture(), stats = f.window.tttNativeStartSample(false);
  f.tick();
  const oldFrame = stats.frame;
  f.window.tttNativeStopSample(stats);
  const idle = f.window.tttNativeStartIdle();
  assert.notEqual(idle, stats);
  assert.equal(stats.active, false);
  assert.equal(f.pending.size, 0);
  f.stale(oldFrame);
  assert.equal(f.pending.size, 0);
  assert.equal(stats.frame, 0);
  assert.equal(stats.frameGaps.length, 0);
  assert.equal(idle.active, true);
  assert.equal(idle.frameGaps, undefined);
});

test('a replaced sample keeps exactly one sampler even if its old callback arrives', () => {
  const f = fixture(), previous = f.window.tttNativeStartSample(false);
  f.tick();
  const oldFrame = previous.frame;
  const current = f.window.tttNativeStartSample(false);
  assert.equal(previous.active, false);
  assert.equal(previous.frame, 0);
  assert.equal(f.pending.size, 1);
  f.stale(oldFrame);
  assert.equal(f.pending.size, 1);
  assert.equal(previous.frameGaps.length, 0);
  assert.equal(f.window.tttNativeProbe, current);
  f.tick();f.tick();
  assert.deepEqual([...current.frameGaps], [16]);
  assert.equal(f.pending.size, 1);
  f.window.tttNativeStopSample();
  assert.equal(f.pending.size, 0);
});

test('sample frame history is bounded and stopping is idempotent', () => {
  const f = fixture(), stats = f.window.tttNativeStartSample(false);
  for(let frame = 0;frame < 3000;frame++)f.tick();
  assert.equal(stats.frameGaps.length, 2048);
  assert.equal(stats.frameSamplesDropped, 951);
  assert.equal(f.pending.size, 1);
  f.window.tttNativeStopSample();f.window.tttNativeStopSample();
  assert.equal(f.pending.size, 0);
  assert.equal(stats.frame, 0);
});

test('four phases partition the single sample buffer, counters, and dropped frames', () => {
  const f = fixture(), stats = f.window.tttNativeStartSample(true);
  const phases = ['resize', 'scroll', 'zoom', 'drain'];
  for(let index = 0;index < phases.length;index++) {
    if(index)f.window.tttNativeMarkPhase(phases[index]);
    stats.rescale += index + 1;stats.rerender += index;stats.viewport += 2;
    stats.rescaleSyncMs += index + 0.5;stats.rerenderSyncMs += index + 0.25;
    for(let frame = 0;frame < 600;frame++)f.tick(16 + index);
  }
  f.window.tttNativeClosePhase();f.window.tttNativeStopSample();
  assert.deepEqual([...stats.phases].map(phase => phase.phase), phases);
  for(const name of ['rescale', 'rerender', 'viewport', 'rescaleSyncMs', 'rerenderSyncMs']) {
    assert.equal(stats.phases.reduce((total, phase) => total + phase[name], 0), stats[name]);
  }
  assert.equal(stats.phases.reduce((total, phase) => total + phase.frameCount, 0), stats.frameGaps.length);
  assert.equal(stats.phases.reduce((total, phase) => total + phase.frameSamplesDropped, 0), stats.frameSamplesDropped);
  assert.ok(stats.phases.every(phase => phase.frameGapMaxMs >= phase.frameGapP95Ms));
  assert.throws(() => f.window.tttNativeMarkPhase('resize'), /Invalid bounded native phase order/);
  assert.equal(stats.phases.length, 4);
  assert.equal(f.pending.size, 0);
});

test('phase diagnostics are inert when disabled and reject out-of-order names', () => {
  const f = fixture(), stats = f.window.tttNativeStartSample(false);
  f.window.tttNativeMarkPhase('invalid');f.window.tttNativeClosePhase();
  assert.equal(stats.phases, undefined);
  assert.equal(f.pending.size, 1);
  f.window.tttNativeStartSample(true);
  assert.throws(() => f.window.tttNativeMarkPhase('drain'), /Invalid bounded native phase order/);
});


test('counter-only idle admission rejects each kind of renderer work, including rescale', () => {
  for(const name of ['rescale', 'rerender', 'viewport']) {
    const f = fixture(), idle = f.window.tttNativeStartIdle();
    idle[name]++;
    const result = f.window.tttNativeFinishIdle();
    assert.equal(result[name], 1);
    assert.equal(result.quiet, false, name);
    assert.equal(idle.active, false);
    assert.equal(f.pending.size, 0);
  }
});

test('quiet idle observation keeps completed samplers inactive and reports all zero counters', () => {
  const f = fixture(), sample = f.window.tttNativeStartSample(false);
  f.tick();
  const oldFrame = sample.frame;
  const idle = f.window.tttNativeStartIdle();
  f.stale(oldFrame);
  const result = f.window.tttNativeFinishIdle();
  assert.equal(result.rescale, 0);
  assert.equal(result.rerender, 0);
  assert.equal(result.viewport, 0);
  assert.equal(result.quiet, true);
  assert.equal(sample.active, false);
  assert.equal(idle.active, false);
  assert.equal(f.pending.size, 0);
});
