using System;
using System.Collections.Generic;
using System.Text;

namespace s3b
{
    public class Model : Dictionary<string, object>
    {
        public Model() { }

        public string tableName { get; set; }

        // this is in a base class, skipped that bit for clairty
        protected object getPropValue(string propName)
        {
            propName = propName.Replace("get_", "").Replace("set_", "");
            object o;
            try
            {
                o = this[propName];
            }
            catch(KeyNotFoundException)
            {
                o = new object();
            }
            return o;
        }

        protected void setPropValue(string propName, object value)
        {
            if (value == null)
            {
                value = string.Empty;
            }

            //propName = propName.Replace("get_", "").Replace("set_", "");
            propName = propName.Substring(4);
            if (this.ContainsKey(propName))
            {
                this[propName] = value;
            }
            else
            {
                this.Add(propName, value);
            }
        }
    }
}
