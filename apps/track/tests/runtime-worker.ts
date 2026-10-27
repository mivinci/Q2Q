// Test-only Worker: no production route, bindings, or deployment configuration.
import { runContract } from './contract-suite.ts';

export default {
  async fetch(): Promise<Response> { return Response.json(await runContract()); }
};
