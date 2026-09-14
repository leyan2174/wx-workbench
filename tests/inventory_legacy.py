"""开发工具：静态盘点受版本控制的旧实现，不导入生产模块或读取用户资料。"""

import ast
import hashlib
import json
from pathlib import Path
import subprocess


def describe(path, relative):
    source = path.read_text(encoding='utf-8-sig')
    result = {'path': relative, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
              'lines': len(source.splitlines()), 'migration': 'requires-review'}
    if path.suffix == '.js':
        result['language'] = 'javascript'
        result['note'] = '需单独核对导出接口、WASM 交互和运行时；未按文本正则推断完整接口。'
        return result
    tree = ast.parse(source, filename=relative)
    result.update(language='python', symbols=[], arguments=[], imports=[])

    class Symbols(ast.NodeVisitor):
        def __init__(self):
            self.parents = []

        def definition(self, node, kind):
            name = '.'.join([*self.parents, node.name])
            result['symbols'].append({'name': name, 'kind': kind, 'line': node.lineno,
                                      'decorators': [ast.unparse(d) for d in node.decorator_list],
                                      'migration': 'requires-review'})
            self.parents.append(node.name)
            self.generic_visit(node)
            self.parents.pop()

        def visit_FunctionDef(self, node): self.definition(node, 'function')
        def visit_AsyncFunctionDef(self, node): self.definition(node, 'async-function')
        def visit_ClassDef(self, node): self.definition(node, 'class')

    Symbols().visit(tree)
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == 'add_argument':
            result['arguments'].append({'line': node.lineno, 'declaration': ast.unparse(node)})
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            result['imports'].append(ast.unparse(node))
    result['imports'] = sorted(set(result['imports']))
    return result


def main():
    root = Path(__file__).resolve().parents[1]
    tracked = subprocess.check_output(['git', 'ls-files', '-z', 'vendor/wechat-decrypt'],
                                     cwd=root).decode('utf-8').split('\0')
    files = [p for p in tracked if p and Path(p).suffix in {'.py', '.js'}
             and 'tests' not in Path(p).parts and (root / p).is_file()]
    modules = [describe(root / p, p) for p in sorted(files)]
    output = root / 'docs' / 'legacy-capability-inventory.json'
    payload = {'schema_version': 1, 'scope': '全部受版本控制的 wechat-decrypt Python/JavaScript 生产源码',
               'note': '这是迁移起点清单，不是完成率报告；函数可合并重构，但每项功能必须有去向和验收。',
               'modules': modules}
    output.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(f'Modules: {len(modules)}')
    print(f'Python symbols: {sum(len(m.get("symbols", [])) for m in modules)}')
    print(f'CLI argument declarations: {sum(len(m.get("arguments", [])) for m in modules)}')
    print(f'Source lines: {sum(m["lines"] for m in modules)}')
    print(f'Inventory: {output}')


if __name__ == '__main__':
    main()
