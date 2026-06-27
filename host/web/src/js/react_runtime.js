(function () {
  function flatten(input, out) {
    for (var i = 0; i < input.length; i += 1) {
      var item = input[i];
      if (Array.isArray(item)) flatten(item, out);
      else if (item !== null && item !== undefined && item !== false && item !== true) out.push(item);
    }
    return out;
  }

  function createElement(type, props) {
    var children = flatten(Array.prototype.slice.call(arguments, 2), []);
    return { type: type, props: props || {}, children: children };
  }

  function useState(initial) {
    var root = window.__TERRANE_REACT_CURRENT_ROOT__;
    if (!root) throw new Error("React.useState called outside render");
    var index = root.hookIndex++;
    if (root.hooks.length <= index) {
      root.hooks.push(typeof initial === "function" ? initial() : initial);
    }
    function setState(next) {
      var value = typeof next === "function" ? next(root.hooks[index]) : next;
      root.hooks[index] = value;
      root.render(root.element);
    }
    return [root.hooks[index], setState];
  }

  window.React = {
    createElement: createElement,
    Fragment: "TERRANE_REACT_FRAGMENT",
    useState: useState
  };
})();
