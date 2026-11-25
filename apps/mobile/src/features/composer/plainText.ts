// Native contenteditable uses both text newlines and block elements. A final BR
// is the browser's empty-line caret placeholder, rather than another newline.
export function editorText(node: Node): string {
  if (node.nodeType === 3) return node.textContent || '';
  const children = Array.from(node.childNodes);
  const block = (child?: Node) => child?.nodeName === 'DIV' || child?.nodeName === 'P';
  return children.map((child, index) => {
    const previous = children[index - 1];
    const separator = index > 0 && (block(child) || block(previous)) && previous?.nodeName !== 'BR' ? '\n' : '';
    const value = child.nodeName === 'BR' ? (index < children.length - 1 ? '\n' : '') : editorText(child);
    return separator + value;
  }).join('');
}
