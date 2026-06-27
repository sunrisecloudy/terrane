const ReactGlobal = window.React;
if (!ReactGlobal) throw new Error('Terrane React runtime is missing');
export const Fragment = ReactGlobal.Fragment;
export function jsx(type, props, key) { return ReactGlobal.createElement(type, key == null ? props : Object.assign({}, props, { key })); }
export const jsxs = jsx;
export const jsxDEV = jsx;
