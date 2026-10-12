import { createRoot } from 'react-dom/client';
import './styles/fonts.css';
import './styles/tokens.css';
import './styles/app.css';
import { App } from './App';
import { ComposeWindow } from './ComposeWindow';
import { MessageWindow } from './MessageWindow';
import { SettingsProvider } from './lib/settings';
import { limitNativeMenu } from './lib/native-menu';

// Every window, the pop-outs included: each would otherwise offer WebKit's
// lone Reload over its controls, which restarts the window.
limitNativeMenu();

// A popped-out window is the same bundle with one query parameter, not a second
// app. Both still want the settings provider — theme, accent and reading size
// are the user's, not the main window's.
const params = new URLSearchParams(window.location.search);
const draftId = Number(params.get('compose'));
const threadId = Number(params.get('message'));
const composing = Number.isFinite(draftId) && draftId > 0;
// Thread ids are negative for unthreaded mail, so this cannot test for > 0.
const reading = Number.isFinite(threadId) && threadId !== 0;

createRoot(document.getElementById('root')!).render(
  <SettingsProvider>
    {composing ? (
      <ComposeWindow draftId={draftId} />
    ) : reading ? (
      <MessageWindow threadId={threadId} />
    ) : (
      <App />
    )}
  </SettingsProvider>,
);
