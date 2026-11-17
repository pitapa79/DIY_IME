import assert from 'node:assert/strict';

const canonical=value=>value===null||typeof value!=='object'?JSON.stringify(value)
  :Array.isArray(value)?`[${value.map(canonical).join(',')}]`
  :`{${Object.keys(value).sort().map(key=>`${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`;

// The Host ring only appends or evicts its oldest row. Prove that every evicted
// row was already captured, every surviving row is unchanged, and every newly
// appended logical Node call is unique. A positive counter alone is NOT proof.
export function proveObservedNativeRingHistory(snapshots) {
  assert(snapshots.length>0);
  const epoch=snapshots[0].sessionEpoch, owners=new Map();
  assert.equal(typeof epoch,'string');
  for(const status of snapshots) {
    assert.equal(status.sessionEpoch,epoch,'Native history Session changed');
    const present=new Set();
    for(const bundle of status.retainedWorker?.callSamples??[]) {
      assert(Array.isArray(bundle.nodeIds)&&bundle.nodeIds.length>0);
      assert(bundle.nodeIds.every(id=>typeof id==='string'));
      assert.equal(new Set(bundle.nodeIds).size,bundle.nodeIds.length);
      const ownerKey=canonical(bundle.nodeIds), calls=bundle.calls;
      assert(!present.has(ownerKey),'duplicate Native history owner');present.add(ownerKey);
      assert(Number.isSafeInteger(calls.capacity)&&calls.capacity>0);
      assert(Number.isSafeInteger(calls.dropped)&&calls.dropped>=0);
      assert(Array.isArray(calls.samples)&&calls.samples.length<=calls.capacity);
      if(calls.dropped)assert.equal(calls.samples.length,calls.capacity,'evicted ring must remain full');
      let owner=owners.get(ownerKey);
      if(!owner) {
        assert.equal(calls.dropped,0,'Native history began after unobserved loss');
        owner={nodeIds:bundle.nodeIds,capacity:calls.capacity,dropped:0,previous:[],seen:new Set()};
        owners.set(ownerKey,owner);
      }
      assert.equal(calls.capacity,owner.capacity,'Native history capacity changed');
      const evicted=calls.dropped-owner.dropped;
      assert(evicted>=0&&evicted<=owner.previous.length,'Native history contains unobserved loss');
      const retained=owner.previous.slice(evicted), current=calls.samples.map(canonical);
      assert(current.length>=retained.length,'Native history truncated without an eviction count');
      assert.deepEqual(current.slice(0,retained.length),retained,'surviving Native evidence changed');
      for(const sample of calls.samples.slice(retained.length)) {
        assert(Number.isSafeInteger(sample.stage)&&sample.stage>=0&&sample.stage<bundle.nodeIds.length);
        assert(['runId','planVersion','deliveryGeneration'].every(k=>typeof sample[k]==='string'&&sample[k].length>0));
        const identity=canonical([bundle.nodeIds[sample.stage],sample.runId,sample.planVersion,
          sample.deliveryGeneration,sample.inputRevision??null]);
        assert(!owner.seen.has(identity),'duplicate logical Native call in archived history');
        owner.seen.add(identity);
      }
      owner.previous=current;owner.dropped=calls.dropped;
      assert.equal(owner.seen.size,owner.dropped+owner.previous.length,'Native archive accounting mismatch');
    }
    assert([...owners.keys()].every(key=>present.has(key)),'Native owner history disappeared');
  }
  const rows=[...owners.values()].map(owner=>({nodeIds:owner.nodeIds,capacity:owner.capacity,
    observedRows:owner.seen.size,retainedRows:owner.previous.length,evictionsPreviouslyObserved:owner.dropped}));
  return {complete:true,scope:'All evicted Native rows captured in earlier immutable snapshots; counters retained',
    snapshots:snapshots.length,owners:rows};
}
