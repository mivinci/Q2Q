import { test } from 'node:test';
import { checks } from './contract-suite.ts';

for (const check of checks) test(check.name, check.run);
