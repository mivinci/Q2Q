import {test} from 'node:test';
import assert from 'node:assert/strict';
import {memoryState} from '../src/memory-state.ts';
import {StorageError} from '../src/error.ts';
test('memory witness updates are copied and remain unchanged after failed CAS or validation',async()=>{
  const state=memoryState(),record=await state.create({content:{id:`sha256:${'01'.repeat(32)}`,size:524288n,blockSize:262144},targetId:'memory-target'});
  const lease=await state.lease(record.id),bytes=new Uint8Array([81,50,81,87,1,0]);
  try{
    await lease.replace(1,new Uint8Array([1]),new Map([[0,bytes]]));bytes[0]=0;
    assert.equal((await lease.witness!(0))![0],81);
    await assert.rejects(lease.replace(1,new Uint8Array([3]),new Map([[1,bytes]])),value=>value instanceof StorageError&&value.code==='CHECKPOINT_CONFLICT');
    await assert.rejects(lease.replace(2,new Uint8Array([1]),new Map([[1,bytes]])),value=>value instanceof StorageError&&value.code==='INVALID_RANGE');
    assert.equal(await lease.witness!(1),null);assert.equal((await state.read(record.id)).revision,2);
    await lease.replace(2,new Uint8Array([0]));assert.equal(await lease.witness!(0),null);
  }finally{await lease.close();await state.close();}
});
