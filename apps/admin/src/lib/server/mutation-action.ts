import { fail } from '@sveltejs/kit';
import { PlanConfigurationError } from '@q2q/plans';
import { MutationError, type Actor, type SavedOperation } from './mutations.js';

export interface ActionState {
  error: string | null;
  saved: string | null;
  values: Record<string, string>;
}
export async function mutationAction(
  request: Request, url: URL, actor: Actor | undefined,
  fields: string[], mutate: (form: FormData) => Promise<SavedOperation>
) {
  const state: ActionState = { error: null, saved: null, values: {} };
  if (!actor?.id || !actor.email || request.headers.get('origin') !== url.origin) {
    return fail(403, { ...state, error: 'forbiddenChange' });
  }
  let form: FormData;
  try { form = await request.formData(); }
  catch { return fail(400, { ...state, error: 'invalidChanges' }); }
  // Preserve only known form fields. Identity always comes from the Access hook.
  for (const key of fields) {
    const value = form.get(key);
    if (typeof value === 'string') state.values[key] = value.slice(0, 2000);
  }
  try {
    const result = await mutate(form);
    return { ...state, saved: result.requestId, values: {} } as ActionState;
  } catch (cause) {
    if (cause instanceof MutationError) return fail(cause.status, { ...state, error: cause.code });
    if (cause instanceof PlanConfigurationError) return fail(503, { ...state, error: 'unavailable' });
    // A transport error can occur after commit. Preserve the request and payload
    // so the administrator can inspect its audit record or retry idempotently.
    console.error('admin mutation result not confirmed', { requestId: state.values.requestId });
    return fail(503, { ...state, error: 'operationUncertain' });
  }
}
