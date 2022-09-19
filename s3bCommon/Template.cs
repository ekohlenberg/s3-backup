using System;
using System.Collections.Generic;
using System.Text;

namespace s3b{
    public class Template
    {
        string script = string.Empty;

        public Template(string script)
        {
            this.script = script;
        }

        public void setScript( string script)
        {
            this.script = script;
        }

        public string eval(Model parameters)
        {
            StringBuilder sb = new StringBuilder(script);
            eval(parameters, sb);

            return sb.ToString();
        }

        private void eval(Model parameters, StringBuilder sb)
        {
            string initialStr = sb.ToString();

            foreach (string k in parameters.Keys)
            {
                object v = string.Empty;
                parameters.TryGetValue(k, out v);

                string paramName = "$(" + k + ")";
                sb.Replace(paramName, v.ToString());
            }

            string finalStr = sb.ToString();

            if (finalStr != initialStr) eval(parameters, sb); 
                        
        }
    }
}
