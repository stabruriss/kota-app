import '@testing-library/jest-dom/vitest';
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

// Auto-cleanup after each test to avoid stale DOM between cases.
afterEach(() => cleanup());
