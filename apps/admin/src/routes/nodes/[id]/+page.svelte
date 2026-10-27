<script lang="ts">
 import { Textarea } from '@q2q/ui/textarea';
 import {page} from '$app/state';
 import {messages} from '#lib/messages.js';
 import {date} from '#lib/format.js';
 import ResponsiveMetadata from '@q2q/ui/responsive-metadata';
 import MutationForm from '#lib/components/mutation-form.svelte';
 import MutationNotice from '#lib/components/mutation-notice.svelte';
 import type {PageProps} from './$types';
 let {data,form}:PageProps=$props();const text=$derived(messages[page.data.locale]);
 const values=$derived(form?.error?form.values:{});
 const enabled=$derived(values.enabled??(data.node?.enabled?'0':'1'));
 const title=$derived(enabled==='1'?text.resumeDiscovery:text.stopDiscovery);
 const items=$derived(data.node?[
  {label:text.policyStatus,value:data.node.enabled?text.discoverable:text.discoveryStopped},
  {label:text.health,value:data.node.online?text.online:text.offline},
  {label:text.region,value:data.node.region??'—'},
  {label:text.version,value:data.node.version},
  {label:text.advertiseUrl,value:data.node.advertise_url,valueClass:'min-w-0 break-all'},
  {label:text.publicKey,value:data.node.public_key,valueClass:'min-w-0 break-all'},
  {label:text.identityVersion,value:String(data.node.identity_version)},
  {label:text.revision,value:String(data.node.revision)},
  {label:text.last_seen,value:date(data.node.last_seen*1000,page.data.locale)},
  {label:text.accepting_new_connections,value:data.node.accepting_new_connections?text.yes:text.no}
 ]:[]);
</script>
<svelte:head><title>{data.nodeId} · Q2Q Admin</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 pt-5 pb-7 sm:px-7">
 <h1 class="break-words text-2xl font-medium tracking-tight">{data.nodeId}</h1><p class="mt-1 text-sm text-muted-foreground">Punch</p>
 <MutationNotice {form} />
 {#if data.node}
 <section class="mt-6 rounded-xl border bg-card p-4"><ResponsiveMetadata {items} dense wrapperClass="md:grid-cols-2 md:gap-x-6" /></section>
 <section class="mt-6 rounded-xl border bg-card p-4">
  <h2 class="text-base font-medium">{title}</h2><p class="mt-2 mb-4 text-sm leading-relaxed text-muted-foreground">{text.discoveryScope}</p>
  <MutationForm {title} destructive={enabled==='0'} review={input=>`${title}: ${data.nodeId}. ${text.reason}: ${input.get('reason')}. ${text.discoveryScope}`}>
   <input type="hidden" name="requestId" value={values.requestId??data.requestId} />
   <input type="hidden" name="revision" value={values.revision??data.node.revision} />
   <input type="hidden" name="enabled" value={enabled} />
   <label class="block text-sm font-medium">{text.reason}<Textarea name="reason" required maxlength={1000} placeholder={text.reasonHint} value={values.reason??''} class="mt-1.5 min-h-20 resize-y bg-background text-sm"></Textarea></label>
  </MutationForm>
 </section>
 {:else}<p role="status" class="mt-6 rounded-xl border bg-card p-4 text-sm">{text.unavailable}</p>{/if}
 <p class="mt-6 text-xs text-muted-foreground">{text.observedAt} · {date(data.observedAt,page.data.locale)}</p>
</div>
