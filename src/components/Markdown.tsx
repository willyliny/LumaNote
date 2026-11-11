import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { isSafeExternalUrl, openExternal } from '../lib/external';
import './markdown.css';

/**
 * Assistant markdown, rendered to React elements (never innerHTML).
 *  - raw HTML in the source is not interpreted (react-markdown has no rehype-raw here)
 *  - only http/https/mailto links survive; clicks go through openExternal
 *  - remote images are not loaded (they would leak the user's IP and context); alt text is shown
 */
const components: Components = {
  a({ href, children }) {
    if (!href || !isSafeExternalUrl(href)) return <span>{children}</span>;
    return (
      <a
        href={href}
        rel="noopener noreferrer"
        onClick={(e) => {
          e.preventDefault();
          openExternal(href);
        }}
      >
        {children}
      </a>
    );
  },
  img({ alt }) {
    return <span className="md-image-stub">[圖片{alt ? `：${alt}` : ''}]</span>;
  },
  input(props) {
    // GFM task list checkboxes: keep them read-only
    return <input {...props} disabled readOnly />;
  },
};

export function Markdown({ text, streaming }: { text: string; streaming?: boolean }) {
  return (
    <div className={`md ${streaming ? 'md--streaming' : ''}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={components}
        urlTransform={(url) => (isSafeExternalUrl(url) ? url : '')}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
