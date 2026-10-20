const palettes = {
  light: {
    surface: '#ffffff',
    text: '#0f172a',
    node: '#f1f5f9',
    border: '#cbd5e1',
    line: '#64748b',
    core: '#dbeafe',
    coreBorder: '#93c5fd',
    support: '#ede9fe',
    supportBorder: '#c4b5fd',
    group: '#f8fafc',
  },
  dark: {
    surface: '#0f172a',
    text: '#e2e8f0',
    node: '#1e293b',
    border: '#475569',
    line: '#94a3b8',
    core: '#172554',
    coreBorder: '#3b82f6',
    support: '#2e1065',
    supportBorder: '#8b5cf6',
    group: '#111c30',
  },
};

// Flowcharts can use `class Client,Peer core` and `class Cache,Stats support`.
// Keep semantic roles in Markdown and all theme-specific colors here.
export function diagramConfig(dark) {
  const p = palettes[dark ? 'dark' : 'light'];
  return {
    startOnLoad: false,
    securityLevel: 'loose',
    theme: 'base',
    themeVariables: {
      darkMode: dark,
      background: p.surface,
      fontFamily: 'ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
      fontSize: '14px',
      primaryColor: p.node,
      primaryTextColor: p.text,
      primaryBorderColor: p.border,
      secondaryColor: p.core,
      secondaryTextColor: p.text,
      secondaryBorderColor: p.coreBorder,
      tertiaryColor: p.support,
      tertiaryTextColor: p.text,
      tertiaryBorderColor: p.supportBorder,
      lineColor: p.line,
      textColor: p.text,
      mainBkg: p.node,
      nodeBorder: p.border,
      clusterBkg: p.group,
      clusterBorder: p.border,
      edgeLabelBackground: p.surface,
      actorBkg: p.core,
      actorBorder: p.coreBorder,
      actorTextColor: p.text,
      actorLineColor: p.border,
      signalColor: p.line,
      signalTextColor: p.text,
      labelBoxBkgColor: p.support,
      labelBoxBorderColor: p.supportBorder,
      labelTextColor: p.text,
      loopTextColor: p.text,
      noteBkgColor: p.support,
      noteBorderColor: p.supportBorder,
      noteTextColor: p.text,
      activationBkgColor: p.node,
      activationBorderColor: p.border,
      sequenceNumberColor: p.surface,
    },
    flowchart: { curve: 'basis', nodeSpacing: 32, rankSpacing: 48, padding: 16 },
    sequence: {
      actorMargin: 40,
      boxMargin: 12,
      messageMargin: 32,
      actorFontSize: 14,
      actorFontWeight: 500,
      noteFontSize: 13,
      messageFontSize: 14,
    },
    themeCSS: `
      .nodeLabel p, .edgeLabel p { margin: 0; line-height: 1.5; }
      .node rect, .actor { rx: 10px; ry: 10px; }
      .cluster rect { rx: 14px; ry: 14px; }
      .node.core > rect, .node.core > polygon, .node.core > circle, .node.core > ellipse, .node.core > path {
        fill: ${p.core}; stroke: ${p.coreBorder};
      }
      .node.support > rect, .node.support > polygon, .node.support > circle, .node.support > ellipse, .node.support > path {
        fill: ${p.support}; stroke: ${p.supportBorder};
      }
      .flowchart-link, .messageLine0, .messageLine1 { stroke-width: 1.25px; }
      .actor-line { stroke-dasharray: 4 5; stroke-width: 1px; }
    `,
  };
}
