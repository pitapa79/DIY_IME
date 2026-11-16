// Test-only original-process/Job protocol; never shipped with the product.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {fileURLToPath} from 'node:url';

export class OwnedLauncherJob {
  constructor(command) {
    this.next=0;this.pending=new Map();this.error='';
    this.child=spawn(process.env.DIY_WINDOWS_TEST_PYTHON || 'python',
      [fileURLToPath(new URL('./owned-launcher-job.py',import.meta.url)),...command],
      {windowsHide:true,stdio:'pipe'});
    this.child.stderr.on('data',data=>{this.error=(this.error+data).slice(-8192);});
    this.started=new Promise((resolve,reject)=>{this.resolveStarted=resolve;this.rejectStarted=reject;});
    this.startTimer=setTimeout(()=>{this.rejectStarted(Error('owned launcher startup timed out'));this.child.kill();},35000);
    this.ended=new Promise(resolve=>{
      this.child.once('error',error=>{this.rejectStarted(error);for(const p of this.pending.values())p.reject(error);resolve(1);});
      this.child.once('close',code=>{
        clearTimeout(this.startTimer);
        const error=Error(`owned launcher wrapper exited ${code}: ${this.error}`);
        this.rejectStarted(error);for(const p of this.pending.values())p.reject(error);
        this.pending.clear();resolve(code);
      });
    });
    createInterface({input:this.child.stdout}).on('line',line=>{
      try {
        assert(line.length<1048576,'bounded Job reply');
        const reply=JSON.parse(line);
        if(reply.event==='started'){clearTimeout(this.startTimer);this.resolveStarted(reply);return;}
        const pending=this.pending.get(reply.id);assert(pending,'unknown Job reply');
        this.pending.delete(reply.id);pending.resolve(reply);
      } catch(error) {
        this.rejectStarted(error);for(const p of this.pending.values())p.reject(error);
        this.child.kill();
      }
    });
  }
  async call(method) {
    const id=String(++this.next);
    let timer;
    const reply=new Promise((resolve,reject)=>{
      this.pending.set(id,{resolve,reject});
      timer=setTimeout(()=>{this.pending.delete(id);reject(Error('bounded Job query timed out'));this.child.kill();},10000);
    });
    try {
      this.child.stdin.write(JSON.stringify({id,method})+'\n');
      const response=await reply;
      assert.equal(response.ok,true,response.error);return response.data;
    } finally {clearTimeout(timer);}
  }
  async close() {
    this.child.stdin.end();
    let timer;
    try {
      const timeout=new Promise(resolve=>{timer=setTimeout(()=>resolve('timeout'),5000);});
      let code=await Promise.race([this.ended,timeout]);
      if(code==='timeout'){this.child.kill();await this.ended;throw Error('owned wrapper cleanup timed out');}
      assert.equal(code,0,this.error);
    } finally {clearTimeout(timer);clearTimeout(this.startTimer);}
  }
}
