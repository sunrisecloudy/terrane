(function () {
  function createRoot(container) {
    return {
      container: container,
      element: null,
      hooks: [],
      hookIndex: 0,
      render: function (element) {
        this.element = element;
        this.hookIndex = 0;
        window.__TERRANE_REACT_CURRENT_ROOT__ = this;
        patch(this.container, this.container.firstChild, element);
        window.__TERRANE_REACT_CURRENT_ROOT__ = null;
      }
    };
  }

  function patch(parent, current, vnode) {
    if (vnode === null || vnode === undefined || vnode === false || vnode === true) {
      return patch(parent, current, "");
    }
    if (typeof vnode === "string" || typeof vnode === "number") {
      if (current && current.nodeType === 3) {
        if (current.nodeValue !== String(vnode)) current.nodeValue = String(vnode);
        return current;
      }
      return replace(parent, current, document.createTextNode(String(vnode)));
    }
    if (Array.isArray(vnode)) {
      patchChildren(parent, vnode);
      return parent;
    }
    if (typeof vnode.type === "function") {
      return patch(parent, current, vnode.type(assign({}, vnode.props, { children: vnode.children })));
    }
    if (vnode.type === window.React.Fragment) {
      patchChildren(parent, vnode.children || []);
      return parent;
    }
    if (
      !current ||
      current.nodeType !== 1 ||
      current.tagName.toLowerCase() !== String(vnode.type).toLowerCase()
    ) {
      current = replace(parent, current, document.createElement(vnode.type));
    }
    setProps(current, vnode.props || {});
    patchChildren(current, vnode.children || []);
    return current;
  }

  function patchChildren(parent, children) {
    for (var i = 0; i < children.length; i += 1) {
      patch(parent, parent.childNodes[i], children[i]);
    }
    while (parent.childNodes.length > children.length) {
      parent.removeChild(parent.lastChild);
    }
  }

  function replace(parent, current, next) {
    if (current) parent.replaceChild(next, current);
    else parent.appendChild(next);
    return next;
  }

  function renderNode(vnode) {
    if (vnode === null || vnode === undefined || vnode === false || vnode === true) {
      return document.createTextNode("");
    }
    if (typeof vnode === "string" || typeof vnode === "number") {
      return document.createTextNode(String(vnode));
    }
    if (Array.isArray(vnode)) {
      var fragment = document.createDocumentFragment();
      vnode.forEach(function (child) { fragment.appendChild(renderNode(child)); });
      return fragment;
    }
    if (typeof vnode.type === "function") {
      return renderNode(vnode.type(assign({}, vnode.props, { children: vnode.children })));
    }
    if (vnode.type === window.React.Fragment) {
      return renderNode(vnode.children);
    }

    var element = document.createElement(vnode.type);
    setProps(element, vnode.props || {});
    (vnode.children || []).forEach(function (child) {
      element.appendChild(renderNode(child));
    });
    return element;
  }

  function setProps(element, props) {
    var prev = element.__terraneProps || {};
    Object.keys(prev).forEach(function (name) {
      if (name === "children" || props[name] !== undefined) return;
      if (name.slice(0, 2) === "on" && typeof prev[name] === "function") {
        element.removeEventListener(name.slice(2).toLowerCase(), prev[name]);
      } else if (name === "className") {
        element.removeAttribute("class");
      } else if (name === "htmlFor") {
        element.removeAttribute("for");
      } else if (name !== "style") {
        element.removeAttribute(name);
      }
    });
    Object.keys(props).forEach(function (name) {
      if (name === "children" || props[name] === null || props[name] === undefined) return;
      if (name === "className") {
        element.setAttribute("class", props[name]);
      } else if (name === "htmlFor") {
        element.setAttribute("for", props[name]);
      } else if (name === "style" && typeof props[name] === "object") {
        Object.keys(props[name]).forEach(function (key) {
          if (key.indexOf("--") === 0) element.style.setProperty(key, props[name][key]);
          else element.style[key] = props[name][key];
        });
      } else if (name === "value" || name === "checked") {
        if (element[name] !== props[name]) element[name] = props[name];
      } else if (name.slice(0, 2) === "on" && typeof props[name] === "function") {
        if (prev[name]) element.removeEventListener(name.slice(2).toLowerCase(), prev[name]);
        element.addEventListener(name.slice(2).toLowerCase(), props[name]);
      } else if (props[name] === true) {
        element.setAttribute(name, "");
      } else if (props[name] !== false) {
        element.setAttribute(name, String(props[name]));
      }
    });
    element.__terraneProps = props;
  }

  function assign(target) {
    for (var i = 1; i < arguments.length; i += 1) {
      var source = arguments[i] || {};
      Object.keys(source).forEach(function (key) {
        target[key] = source[key];
      });
    }
    return target;
  }

  window.ReactDOM = { createRoot: createRoot };
})();
