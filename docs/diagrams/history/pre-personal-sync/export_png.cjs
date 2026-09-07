// 使用已有 Playwright 浏览器渲染中文；不访问网络或账号文件。
const fs = require('fs');
const path = require('path');
const { chromium } = require(process.argv[2] || 'playwright');

(async () => {
  const browser = await chromium.launch({ headless: true, channel: 'msedge' });
  const names = ['runtime-current', 'sns-cache-publish', 'asr-wasm-wiring', 'mcp-voice-flow'];
  const selected = process.argv[3];
  if (selected && !names.includes(selected)) throw new Error('Unknown diagram: ' + selected);
  const reportPath = path.join(__dirname, 'render-report.json');
  const report = selected && fs.existsSync(reportPath)
    ? JSON.parse(fs.readFileSync(reportPath, 'utf8')).filter(item => item.name !== selected)
    : [];
  try {
    for (const name of names.filter(name => !selected || name === selected)) {
      const svg = fs.readFileSync(path.join(__dirname, name + '.svg'), 'utf8');
      const height = Number(svg.match(/height="(\d+)"/)[1]);
      const page = await browser.newPage({ viewport: { width: 1640, height }, deviceScaleFactor: 2 });
      await page.route('**/*', route => route.abort());
      await page.setContent('<body style="margin:0">' + svg + '</body>');
      await page.evaluate(() => document.fonts.ready);
      const errors = await page.evaluate(() => {
        const errors = [];
        const canvas = document.querySelector('svg').viewBox.baseVal;
        const texts = [...document.querySelectorAll('text')].map(el => ({ el, b: el.getBBox() }));
        const nodes = [...document.querySelectorAll('[data-graph-role="node"]')].map(el => ({ el, b: el.getBBox() }));
        for (const { el, b } of texts) {
          if (b.x < 0 || b.y < 0 || b.x + b.width > canvas.width || b.y + b.height > canvas.height)
            errors.push('canvas text overflow: ' + el.textContent);
          const id = el.dataset.owner;
          if (id) {
            const n = document.getElementById(id).getBBox();
            if (b.x < n.x + 8 || b.x + b.width > n.x + n.width - 8 || b.y < n.y || b.y + b.height > n.y + n.height)
              errors.push('overflow: ' + el.textContent);
          }
          for (const node of nodes) {
            if (node.el.id === id) continue;
            const n = node.b;
            if (b.x < n.x + n.width && b.x + b.width > n.x && b.y < n.y + n.height && b.y + b.height > n.y)
              errors.push('text/node collision: ' + el.textContent);
          }
        }
        for (let i = 0; i < texts.length; i++) for (let j = i + 1; j < texts.length; j++) {
          const a = texts[i].b, b = texts[j].b;
          if (a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y)
            errors.push('text collision: ' + texts[i].el.textContent + ' / ' + texts[j].el.textContent);
        }
        for (const edge of document.querySelectorAll('[data-graph-role="edge"]')) {
          for (let length = 0; length <= edge.getTotalLength(); length += 2) {
            const p = edge.getPointAtLength(length);
            for (const { el, b } of nodes) {
              if (el.id === edge.dataset.source || el.id === edge.dataset.target) continue;
              if (p.x > b.x && p.x < b.x + b.width && p.y > b.y && p.y < b.y + b.height)
                errors.push('edge/node collision: ' + el.id);
            }
            for (const { el, b } of texts) {
              if (p.x > b.x - 1 && p.x < b.x + b.width + 1 && p.y > b.y - 1 && p.y < b.y + b.height + 1)
                errors.push('edge/text collision: ' + el.textContent);
            }
          }
        }
        return errors;
      });
      if (errors.length) throw new Error(name + ': ' + errors.join('\n'));
      await page.screenshot({ path: path.join(__dirname, name + '.png') });
      report.push({ name, width: 3280, height: height * 2, text_overflows: 0, canvas_text_overflows: 0, text_collisions: 0, text_node_collisions: 0, edge_text_collisions: 0, edge_node_collisions: 0, renderer: 'Playwright / Edge' });
      console.log(name + ': PNG exported; measured text checks passed');
      await page.close();
    }
    fs.writeFileSync(path.join(__dirname, 'render-report.json'), JSON.stringify(report, null, 2) + '\n');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
