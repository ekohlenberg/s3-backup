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
        public string eval(Model parameters)
        {
            StringBuilder sb = new StringBuilder(script);

            foreach (string k in parameters.Keys)
            {
                object v = string.Empty;
                parameters.TryGetValue(k, out v);

                string paramName = "$(" + k + ")";
                sb.Replace(paramName, v.ToString());
            }

            return sb.ToString();
        }
    }
}
