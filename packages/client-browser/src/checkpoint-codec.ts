import { StorageError } from './error.ts';
import type { Checkpoint, CheckpointBinding } from './checkpoint-types.ts';

export const MAX_BLOCKS = 524288;
export const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
export interface StoredCheckpoint {
  namespace: string;
  version: 1;
  id: string;
  revision: number;
  contentId: `sha256:${string}`;
  size: string;
  blockSize: number;
  targetId: string;
  committed: Uint8Array;
  updatedAt: number;
}
export function blockCount(binding: CheckpointBinding): number {
  const { size, blockSize } = binding.content;
  return Number((size + BigInt(blockSize) - 1n) / BigInt(blockSize));
}
export function validateBinding(binding: CheckpointBinding): void {
  const { id, size, blockSize } = binding.content;
  if (typeof id !== 'string' || typeof binding.targetId !== 'string' || !/^sha256:[0-9a-f]{64}$/.test(id) || typeof size !== 'bigint' || size < 0n
    || size > BigInt(Number.MAX_SAFE_INTEGER) || !Number.isSafeInteger(blockSize)
    || blockSize < 262144 || blockSize > 4194304 || (blockSize & (blockSize - 1)) !== 0
    || !/^[a-zA-Z0-9._:-]{1,128}$/.test(binding.targetId)) throw new StorageError('CHECKPOINT_MISMATCH');
  if (blockCount(binding) > MAX_BLOCKS) throw new StorageError('RESOURCE_LIMIT');
}
export function hasBlock(bitmap: Uint8Array, index: number): boolean {
  return (bitmap[index >> 3]! & (1 << (index & 7))) !== 0;
}
export function setBlock(bitmap: Uint8Array, index: number): void { bitmap[index >> 3]! |= 1 << (index & 7); }
export function validateBitmap(bitmap: Uint8Array, count: number): void {
  if (!(bitmap instanceof Uint8Array) || bitmap.length !== Math.ceil(count / 8)
    || (count % 8 !== 0 && ((bitmap[bitmap.length - 1] ?? 0) >> (count % 8)) !== 0)) {
    throw new StorageError('CHECKPOINT_CORRUPT');
  }
}
export function decodeCheckpoint(value: unknown, namespace: string, id?: string): Checkpoint {
  if (!value) throw new StorageError('CHECKPOINT_NOT_FOUND');
  const record = value as StoredCheckpoint;
  if (record.version !== 1) throw new StorageError('CHECKPOINT_VERSION');
  if (record.namespace !== namespace || !UUID.test(record.id) || (id !== undefined && record.id !== id)
    || !Number.isSafeInteger(record.revision) || record.revision < 1
    || typeof record.size !== 'string' || !/^(0|[1-9][0-9]{0,15})$/.test(record.size)
    || !Number.isSafeInteger(record.updatedAt) || record.updatedAt < 0) throw new StorageError('CHECKPOINT_CORRUPT');
  const binding = { content: { id: record.contentId, size: BigInt(record.size), blockSize: record.blockSize }, targetId: record.targetId };
  try { validateBinding(binding); validateBitmap(record.committed, blockCount(binding)); }
  catch { throw new StorageError('CHECKPOINT_CORRUPT'); }
  let committedBytes = 0n;
  for (let index = 0; index < blockCount(binding); index++) {
    if (hasBlock(record.committed, index)) {
      const remaining = binding.content.size - BigInt(index * binding.content.blockSize);
      committedBytes += remaining < BigInt(binding.content.blockSize) ? remaining : BigInt(binding.content.blockSize);
    }
  }
  return Object.freeze({ ...binding, content: Object.freeze(binding.content), id: record.id,
    revision: record.revision, committed: new Uint8Array(record.committed), committedBytes, updatedAt: record.updatedAt });
}
export function encodeCheckpoint(namespace: string, checkpoint: Checkpoint): StoredCheckpoint {
  return { namespace, version: 1, id: checkpoint.id, revision: checkpoint.revision,
    contentId: checkpoint.content.id, size: checkpoint.content.size.toString(), blockSize: checkpoint.content.blockSize,
    targetId: checkpoint.targetId, committed: new Uint8Array(checkpoint.committed), updatedAt: checkpoint.updatedAt };
}


export function copyWitnesses(input:ReadonlyMap<number,Uint8Array>|undefined,count:number,bitmap:Uint8Array):Map<number,Uint8Array>{
  const result=new Map<number,Uint8Array>();if(!input)return result;
  if(input.size>1024)throw new StorageError('RESOURCE_LIMIT');
  let seen=0;
  for(const [index,bytes]of input){
    if(++seen>1024)throw new StorageError('RESOURCE_LIMIT');
    if(!Number.isSafeInteger(index)||index<0||index>=count||!hasBlock(bitmap,index))throw new StorageError('INVALID_RANGE');
    if(!(bytes instanceof Uint8Array)||bytes.length<6||bytes.length>640)throw new StorageError('INVALID_RANGE');
    result.set(index,bytes.slice());
  }
  return result;
}
export function decodeWitness(value:unknown,namespace:string,id:string,index:number):Uint8Array|null{
  if(!value||typeof value!=='object')return null;
  const record=value as {version?:unknown;namespace?:unknown;id?:unknown;index?:unknown;bytes?:unknown};
  if(typeof record.version==='number'&&record.version>1)throw new StorageError('CHECKPOINT_VERSION');
  if(record.version!==1||record.namespace!==namespace||record.id!==id||record.index!==index||!(record.bytes instanceof Uint8Array)||record.bytes.length<6||record.bytes.length>640)return null;
  return record.bytes.slice();
}
