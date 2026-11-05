import React from 'react';
import ReactDOM from 'react-dom/client';
import { ReactFlowProvider } from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import App from './App';
import { DistributionShell } from './DistributionShell';
import './styles.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode><ReactFlowProvider>{import.meta.env.VITE_DISTRIBUTION === '1' ? <DistributionShell><App externalOnly /></DistributionShell> : <App externalOnly />}</ReactFlowProvider></React.StrictMode>,
);
