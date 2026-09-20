// One line the person is looking at: a refusal, or what was done. Never a browser dialog.
export default function Notice({ text, bad = false }) {
  if (!text) return null
  return <p role="alert" className={`rounded-md border px-3 py-2 text-sm ${bad ? 'border-terra-600/30 bg-terra-100 text-terra-600' : 'border-green-600/30 bg-green-100 text-green-700'}`}>{text}</p>
}
