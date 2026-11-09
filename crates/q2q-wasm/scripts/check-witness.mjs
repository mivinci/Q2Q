import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {SeedBuilder,PeerProofVerifier} from '../pkg/q2q_wasm.js';
const hash=(...parts)=>{const value=createHash('sha256');for(const part of parts)value.update(part);return value.digest();};
for(const count of [1,3,5]){
  const bytes=Buffer.alloc((count-1)*262144+17);for(let i=0;i<bytes.length;i++)bytes[i]=i%251;
  const builder=new SeedBuilder(bytes.length);builder.update(bytes);const material=builder.finish('witness');const seed=material.seed_bytes();material.free();
  const blocks=Array.from({length:count},(_,i)=>bytes.subarray(i*262144,Math.min(bytes.length,(i+1)*262144)));
  const layers=[blocks.map(block=>hash(Buffer.from([0]),block))];
  while(layers.at(-1).length>1){const last=layers.at(-1),next=[];for(let i=0;i<last.length;i+=2)next.push(i+1<last.length?hash(Buffer.from([1]),last[i],last[i+1]):last[i]);layers.push(next);}
  const verifier=new PeerProofVerifier(seed);
  for(let index=0;index<count;index++){
    const nodes=[];for(let level=0,pos=index;level<layers.length-1;level++,pos=Math.floor(pos/2)){const sibling=pos^1;if(sibling<layers[level].length)nodes.push({level,index:sibling,hash:Array.from(layers[level][sibling])});}
    const frame=JSON.stringify({v:1,message:{type:'proof',block_index:index,count:1,proof:{leaf_hashes:[Array.from(layers[0][index])],nodes}}});
    const witness=verifier.witness(index,blocks[index],frame);
    if(index===0&&count>1){
      const expanded=[];for(const node of nodes){if(node.level===0)expanded.push(node);else{const level=node.level-1;for(const position of [node.index*2,node.index*2+1])if(position<layers[level].length)expanded.push({level,index:position,hash:Array.from(layers[level][position])});}}
      const alternate=JSON.stringify({v:1,message:{type:'proof',block_index:index,count:1,proof:{leaf_hashes:[],nodes:expanded.reverse()}}});
      assert.deepEqual(verifier.witness(index,blocks[index],alternate),witness);
    }assert.ok(witness.length<=640&&verifier.verify(index,blocks[index],witness));
    const wrong=Buffer.from(blocks[index]);wrong[0]^=1;assert.equal(verifier.verify(index,wrong,witness),false);assert.throws(()=>verifier.witness(index,wrong,frame));
    assert.equal(verifier.verify(index,blocks[index],witness.subarray(0,witness.length-1)),false);
    const mutated=witness.slice();mutated[0]^=1;assert.equal(verifier.verify(index,blocks[index],mutated),false);
    if(count>1)assert.equal(verifier.verify((index+1)%count,blocks[(index+1)%count],witness),false);
    const attack=JSON.stringify({v:1,message:{type:'proof',block_index:index,count:1,proof:{leaf_hashes:[],nodes:[{level:layers.length-1,index:0,hash:Array.from(layers.at(-1)[0])}]}}});
    assert.throws(()=>verifier.witness(index,wrong,attack));
  }
  verifier.free();
}
console.log('PASS independent odd-tree peer proofs, compact witnesses, damaged bytes, wrong indices and root-shadow rejection');
