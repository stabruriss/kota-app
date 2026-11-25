import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { recordSystemLoadPageLoadedAt } from './lib/system-load';

recordSystemLoadPageLoadedAt();

const container = document.getElementById('root');
if (!container) throw new Error('root container missing');

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
