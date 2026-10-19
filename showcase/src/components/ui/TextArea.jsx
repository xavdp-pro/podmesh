import { forwardRef } from 'react'
import { INPUT_CLASS, inputStateClass } from './TextInput'

/** Multi-line input, same look as TextInput. `rows` defaults to 4; resizes vertically only. */
const TextArea = forwardRef(function TextArea({ invalid = false, className = '', rows = 4, ...props }, ref) {
  return <textarea ref={ref} rows={rows} className={`${INPUT_CLASS} resize-y ${inputStateClass(invalid)} ${className}`} {...props} />
})

export default TextArea
