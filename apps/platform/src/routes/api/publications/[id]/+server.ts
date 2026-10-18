import type { RequestHandler } from './$types';
import { uploadUser } from '#lib/server/upload-api.js';
import { requirePublication, withdrawPublication } from '#lib/server/publications.js';

export const GET: RequestHandler = async (event) => Response.json(await requirePublication(event.params.id, uploadUser(event)));
export const DELETE: RequestHandler = async (event) => Response.json(await withdrawPublication(event.params.id, uploadUser(event)));
