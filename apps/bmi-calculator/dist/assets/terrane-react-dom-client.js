const ReactDOMGlobal = window.ReactDOM;
if (!ReactDOMGlobal) throw new Error('Terrane ReactDOM runtime is missing');
export default ReactDOMGlobal;
export const createRoot = ReactDOMGlobal.createRoot;
export const hydrateRoot = ReactDOMGlobal.hydrateRoot;
