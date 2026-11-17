// Test-only controlled Worker fault; no execution authority comes from this data.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {fileURLToPath} from 'node:url';

export class HeldPrimitiveWorker {
  constructor({python,pid,workspace,digest}) {
    assert(Number.isSafeInteger(pid)&&pid>0);
    this.pid=pid;this.header=null;this.release=null;this.stderr='';this.parseError=null;
    this.child=spawn(python,[fileURLToPath(new URL('./hold-owned-primitive-worker.py',import.meta.url)),
      String(pid),workspace,digest],{windowsHide:true,stdio:'pipe'});
    this.child.stdin.on('error',()=>{});
    this.child.stderr.on('data',data=>{this.stderr=(this.stderr+data).slice(-8192);});
    this.started=new Promise((resolve,reject)=>{this.resolveStarted=resolve;this.rejectStarted=reject;});
    this.started.catch(()=>{});
    this.startTimer=setTimeout(()=>{
      this.parseError=Error('owned Worker hold setup exceeded bound');
      this.rejectStarted(this.parseError);this.child.stdin.end();
    },16000);
    this.completion=new Promise(resolve=>{
      this.child.once('error',error=>{
        this.parseError=error;this.rejectStarted(error);clearTimeout(this.startTimer);
        resolve({code:null,error,header:this.header,release:this.release});
      });
      this.child.once('close',code=>{
        clearTimeout(this.startTimer);
        if(!this.header)this.rejectStarted(Error(`hold helper exited ${code}: ${this.stderr}`));
        resolve({code,error:this.parseError,header:this.header,release:this.release,stderr:this.stderr});
      });
    });
    createInterface({input:this.child.stdout}).on('line',line=>{
      try {
        assert(line.length<4096,'bounded hold-helper message');
        const value=JSON.parse(line);
        if(value.event==='held') {
          assert(!this.header && !this.release);
          assert.equal(value.pid,pid);
          assert(/^\d{1,20}$/.test(value.creationFileTime));
          assert(Number.isInteger(value.threads)&&value.threads>=1&&value.threads<=128);
          this.header=value;clearTimeout(this.startTimer);this.resolveStarted(value);
        } else {
          assert.equal(value.event,'released');assert(this.header&&!this.release);
          assert(['original-exit','timeout','eof-or-release'].includes(value.reason));
          assert.equal(value.threads,this.header.threads);
          assert.equal(typeof value.originalHandleSignalled,'boolean');
          assert.equal(typeof value.forcedCleanup,'boolean');
          assert(Number.isInteger(value.resumed)&&value.resumed>=0&&value.resumed<=value.threads);
          this.release=value;
        }
      } catch(error) {
        this.parseError=error;this.rejectStarted(error);this.child.stdin.end();
      }
    });
  }
  async expectOriginalExit() {
    const result=await this.completion;
    assert.equal(result.code,0,`hold helper failed: ${result.stderr}`);
    assert(!result.error,result.error?.message);
    assert(result.header && result.release,'complete held/original-exit evidence required');
    assert.equal(result.release.reason,'original-exit','EOF/timeout is not Host cancellation');
    assert.equal(result.release.originalHandleSignalled,true);
    assert.equal(result.release.forcedCleanup,false,'helper cleanup is not Host cancellation');
    return result;
  }
  async close() {
    this.child.stdin.end();
    let timer;
    try {
      const timeout=new Promise(resolve=>{timer=setTimeout(()=>resolve(null),21000);});
      const result=await Promise.race([this.completion,timeout]);
      if(!result){this.child.kill();await this.completion;throw Error('hold helper cleanup exceeded bound');}
      return result;
    } finally {clearTimeout(timer);clearTimeout(this.startTimer);}
  }
}
