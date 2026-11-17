// Private benchmark evidence only; never stores input/result/sample contents.
import assert from 'node:assert/strict';

export class PrivateReplyEvidence {
  constructor() {
    this.byMethod={};
    this.repeatedSnapshotsChecked=0;
  }
  record(method,line,reply) {
    const row=this.byMethod[method]??={replies:0,jsonBytes:0,maxJsonBytes:0,repliesWithSamples:0};
    const bytes=Buffer.byteLength(line,'utf8'); // JSON line, excluding its delimiter.
    row.replies++;row.jsonBytes+=bytes;row.maxJsonBytes=Math.max(row.maxJsonBytes,bytes);
    const data=reply.data;
    if(Object.hasOwn(data?.retainedWorker??data?.state?.retainedWorker??{},'callSamples'))row.repliesWithSamples++;
  }
  assertExclusive() {
    for(const [method,row] of Object.entries(this.byMethod)) {
      if(method!=='status')assert.equal(row.repliesWithSamples,0,`${method} must not materialize private callSamples`);
    }
  }
  verifyRepeatedStatus(first,second,requiredNodeIds=[]) {
    const samples=first.retainedWorker.callSamples;
    const repeated=second.retainedWorker.callSamples;
    assert.deepEqual(repeated?.map(bundle=>bundle.nodeIds),samples?.map(bundle=>bundle.nodeIds),'explicit retrieval must preserve the owner bindings');
    for(let i=0;i<(samples?.length??0);i++) {
      const before=samples[i].calls,after=repeated[i].calls;
      for(const field of ['samples','dropped','capacity','phaseCapacity','clockScope']) {
        assert.deepEqual(after[field],before[field],`explicit retrieval must preserve settled ${field}`);
      }
      // Original-owner cleanup can append phase events after input callbacks
      // settle. Preserve that evidence rather than demanding a frozen lifetime.
      if(before.phases) {
        const evicted=after.phaseDropped-before.phaseDropped;
        assert(Number.isSafeInteger(evicted)&&evicted>=0,'phase loss must remain accounted');
        const retained=before.phases.slice(Math.min(evicted,before.phases.length));
        assert.deepEqual(after.phases.slice(0,retained.length),retained,'explicit retrieval must not drain phase evidence');
      }
    }
    for(const node of requiredNodeIds) {
      assert(samples?.some(bundle=>bundle.nodeIds.includes(node)&&bundle.calls.samples.length>0),`missing explicit samples for ${node}`);
    }
    this.repeatedSnapshotsChecked++;
  }
}
