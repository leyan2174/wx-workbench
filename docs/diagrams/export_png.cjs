// 从唯一架构正文生成图件；仅使用本地 Mermaid、Playwright 和 Edge。
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { chromium } = require(process.argv[2] || 'playwright');
const bundle = process.argv[3] || process.env.MERMAID_BUNDLE;
const sourcePath = path.resolve(__dirname, '../architecture.md');
const sha = value => crypto.createHash('sha256').update(value).digest('hex');
const source = fs.readFileSync(sourcePath, 'utf8');
const blocks = [...source.matchAll(/^\x60\x60\x60mermaid\s*\r?\n([\s\S]*?)^\x60\x60\x60\s*$/gm)].map((match, index) => ({
  name: 'diagram-' + String(index + 1).padStart(2, '0'),
  title: [...source.slice(0, match.index).matchAll(/^#{1,6} (.+)$/gm)].at(-1)?.[1].trim() || 'Diagram ' + (index + 1),
  sourceLine: source.slice(0, match.index).split('\n').length,
  code: match[1].replace(/\r\n/g, '\n').trim() + '\n',
}));
const aliases = { 'runtime-current': [2], 'current-runtime': [2], 'mcp-voice-flow': [23], 'asr-wasm-wiring': [9, 10], 'sns-cache-publish': [11, 12] };

(async () => {
  if (!bundle || !fs.existsSync(bundle)) throw new Error('Pass an existing local Mermaid bundle as argument 3 or MERMAID_BUNDLE. No downloads.');
  if (blocks.length !== 28) throw new Error('Architecture block count changed; review alias mapping.');
  for (const [number, pattern] of [[2, /flowchart/], [9, /ASR|asr/], [10, /WASM|wasm/], [11, /SNS|sns/], [12, /SNS|sns/], [23, /sequenceDiagram/]]) {
    if (!pattern.test(blocks[number - 1].code)) throw new Error('Unexpected topic at diagram ' + number);
  }
  const report = { source: '../architecture.md', sourceSha256: sha(source), mermaidSha256: sha(fs.readFileSync(bundle)), diagrams: [], aliases, checks: 'Flowchart node overlap and label bounds only; sequence diagrams rendered, not automatically collision-audited.' };
  const browser = await chromium.launch({ headless: true, channel: 'msedge' });
  try {
    const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1 });
    await page.route('**/*', route => route.abort());
    await page.setContent('<html><body style="margin:0;background:white"></body></html>');
    await page.addScriptTag({ path: path.resolve(bundle) });
    await page.evaluate(() => mermaid.initialize({ startOnLoad: false, securityLevel: 'strict' }));
    const rendered = [];
    for (const block of blocks) {
      const result = await page.evaluate(async ({ name, code }) => {
        document.body.replaceChildren();
        const result = await mermaid.render(name, code);
        document.body.innerHTML = result.svg;
        await document.fonts.ready;
        const svg = document.querySelector('svg');
        const box = svg.viewBox.baseVal;
        const width = Math.ceil(box.width), height = Math.ceil(box.height);
        if (!(width > 0 && height > 0)) throw new Error('Empty SVG');
        svg.setAttribute('width', width);
        svg.setAttribute('height', height);
        svg.style.maxWidth = 'none';
        svg.style.display = 'block';
        const nodes = [...svg.querySelectorAll('g.node')].map(el => ({ el, rect: el.getBoundingClientRect() }));
        const overlaps = [], overflows = [];
        for (let i = 0; i < nodes.length; i++) {
          const a = nodes[i].rect;
          for (let j = i + 1; j < nodes.length; j++) {
            const b = nodes[j].rect;
            if (Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1 && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 1)
              overlaps.push([nodes[i].el.id, nodes[j].el.id]);
          }
          const shape = nodes[i].el.querySelector('rect,polygon,circle,ellipse,path');
          const label = nodes[i].el.querySelector('.label');
          if (shape && label) {
            const s = shape.getBoundingClientRect(), l = label.getBoundingClientRect();
            if (l.left < s.left - 1 || l.right > s.right + 1 || l.top < s.top - 1 || l.bottom > s.bottom + 1) overflows.push(nodes[i].el.id);
          }
        }
        return { svg: svg.outerHTML, width, height, nodes: nodes.length, overlaps, overflows };
      }, block);
      if (result.overlaps.length || result.overflows.length) throw new Error(block.name + ': ' + JSON.stringify({ overlaps: result.overlaps, overflows: result.overflows }));
      fs.writeFileSync(path.join(__dirname, block.name + '.mmd'), block.code);
      fs.writeFileSync(path.join(__dirname, block.name + '.svg'), result.svg);
      await page.locator('svg').screenshot({ path: path.join(__dirname, block.name + '.png') });
      const { svg, ...geometry } = result;
      report.diagrams.push({ name: block.name, title: block.title, classification: /历史设计/.test(block.title) ? 'historical-design' : 'current-structure', sourceLine: block.sourceLine, sourceSha256: sha(block.code), ...geometry });
      rendered.push(result);
      console.log(block.name + ': ' + result.width + 'x' + result.height + ', nodes=' + result.nodes + ', overlap=0, label-overflow=0');
    }
    for (const [name, indices] of Object.entries(aliases)) {
      const parts = indices.map(index => rendered[index - 1]);
      if (parts.length === 1) {
        for (const ext of ['mmd', 'svg', 'png']) fs.copyFileSync(path.join(__dirname, blocks[indices[0] - 1].name + '.' + ext), path.join(__dirname, name + '.' + ext));
      } else {
        const width = Math.max(...parts.map(part => part.width));
        let y = 0;
        const children = parts.map(part => {
          const child = part.svg.replace('<svg ', '<svg x="' + (width - part.width) / 2 + '" y="' + y + '" ');
          y += part.height + 32;
          return child;
        });
        const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="' + width + '" height="' + (y - 32) + '" viewBox="0 0 ' + width + ' ' + (y - 32) + '"><rect width="100%" height="100%" fill="white"/>' + children.join('') + '</svg>';
        fs.writeFileSync(path.join(__dirname, name + '.svg'), svg);
        await page.setContent('<body style="margin:0;background:white">' + svg + '</body>');
        await page.evaluate(() => document.fonts.ready);
        await page.locator('body > svg').screenshot({ path: path.join(__dirname, name + '.png') });
      }
      console.log(name + ': synchronized from ' + indices.map(index => blocks[index - 1].name).join(', '));
    }
    if (sha(fs.readFileSync(sourcePath, 'utf8')) !== report.sourceSha256) throw new Error('Architecture changed during rendering; rerun.');
    fs.writeFileSync(path.join(__dirname, 'render-report.json'), JSON.stringify(report, null, 2) + '\n');
    console.log('Complete: ' + blocks.length + ' Mermaid diagrams and ' + Object.keys(aliases).length + ' aliases; no network requests allowed.');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
